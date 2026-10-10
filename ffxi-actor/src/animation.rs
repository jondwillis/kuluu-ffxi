use std::collections::HashMap;

use ffxi_dat::skel_anim::{KeyFrameTransform, SkeletonAnimation};

/// How retail merges one bone's pose between two live motion layers.
///
/// `FFXiMain.dll retail-2026-09` RVA 0x33220: at `t == 1` it copies the incoming
/// quaternion verbatim; otherwise it flips the incoming sign when the dot product is
/// negative (shortest arc) and stores `a*(1-t) + b*t` **raw** — the routine contains no
/// square root or division, so the blended value keeps whatever magnitude that sum
/// produced. Within-clip key interpolation (`ffxi_dat::skel_anim`) is a different routine
/// with no law of this kind recorded, so it keeps normalising.
pub fn merge_layer_rotation(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    if t == 1.0 {
        return b;
    }
    weighted_sum(a, on_side_of(b, a), t)
}

fn weighted_sum(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let inv = 1.0 - t;
    [
        a[0] * inv + b[0] * t,
        a[1] * inv + b[1] * t,
        a[2] * inv + b[2] * t,
        a[3] * inv + b[3] * t,
    ]
}

fn dot4(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// `q` or its negation, whichever lies on `reference`'s side of the sphere: the same rotation either way.
fn on_side_of(q: [f32; 4], reference: [f32; 4]) -> [f32; 4] {
    if dot4(q, reference) < 0.0 {
        q.map(|c| -c)
    } else {
        q
    }
}

/// `|w|` of `q` normalised: 1 at the bind orientation, 0 half a turn away. A sum with no length counts as furthest.
fn nearness_to_bind(q: [f32; 4]) -> f32 {
    let length = dot4(q, q).sqrt();
    if length <= f32::EPSILON {
        0.0
    } else {
        q[3].abs() / length
    }
}

/// The two quaternions one bone's crossfade merges, each kept on the side of the sphere it stood on the tick
/// before. Retail picks the short way round afresh on every sample ([`merge_layer_rotation`]); where the
/// two poses sit about a half turn apart while both move, that pick changes sides between ticks and the bone
/// jumps across mid-blend. Holding the pair keeps the blend on the way it set out ([`HeldArc::set_out`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeldArc {
    outgoing: [f32; 4],
    incoming: [f32; 4],
}

impl HeldArc {
    /// The way a merge sets out: the incoming pose the short way round from the outgoing one. Turning toward
    /// the target ([`TransitionParams::turn_toward_target`]), where the short way swings the bone round behind
    /// both poses it blends between, it sets out the opposite way instead, which comes round through the bind
    /// orientation: the body faces its target, so a left/right side-step crossfade turns through the target.
    fn set_out(outgoing: [f32; 4], incoming: [f32; 4], toward_target: bool) -> Self {
        let short = on_side_of(incoming, outgoing);
        if !toward_target {
            return Self {
                outgoing,
                incoming: short,
            };
        }
        let opposite = short.map(|c| -c);
        let short_mid = nearness_to_bind(weighted_sum(outgoing, short, 0.5));
        let behind_both = short_mid < nearness_to_bind(outgoing).min(nearness_to_bind(incoming));
        let through_bind = nearness_to_bind(weighted_sum(outgoing, opposite, 0.5)) > short_mid;
        Self {
            outgoing,
            incoming: if behind_both && through_bind {
                opposite
            } else {
                short
            },
        }
    }

    fn held(self, outgoing: [f32; 4], incoming: [f32; 4]) -> Self {
        Self {
            outgoing: on_side_of(outgoing, self.outgoing),
            incoming: on_side_of(incoming, self.incoming),
        }
    }

    /// Retail's raw weighted sum along the held pair: a copy of the incoming side at `t == 1`.
    fn rotation(self, t: f32) -> [f32; 4] {
        if t == 1.0 {
            return self.incoming;
        }
        weighted_sum(self.outgoing, self.incoming, t)
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

/// One bone's byte of retail's per-bone motion mask (`FFXiMain.dll retail-2026-09` array `.data
/// 0x1045F028`). The low six bits hold a channel category (read by `and eax,0x3f` at RVA 0x19AA3 and by
/// `test al,0x3f` at RVA 0x1A516 / 0x1A5A7); bit 6 marks a bone already claimed while base layers were
/// sampled (armed inline after every sampled slot at RVA 0x1A50D..0x1A51C, wholesale at RVA 0x19B00) and
/// is consumed only by the queued-motion policy dispatch; bit 7 says the bone accepts blend layers
/// (`test al,al` / `jns` at RVA 0x1A5A3, same test in ApplyPolicy at RVA 0x19A92).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoneMotionMask(u8);

impl BoneMotionMask {
    const CATEGORY_BITS: u8 = 0x3f;
    /// Bit 7: the bone accepts blend layers (crossfades).
    pub const ACCEPTS_BLEND_LAYERS: u8 = 1 << 7;

    /// Every bone of every rig ships like this: shipped clips carry no category kuluu can read yet, and a
    /// gate fed by data nobody has is worse than an open one.
    pub const DEFAULT: Self = Self(1 | Self::ACCEPTS_BLEND_LAYERS);

    /// No channel category and blends refused: nothing kuluu samples can take this bone while it stands
    /// like this.
    pub const BLOCKED: Self = Self(0);

    const fn bits(self) -> u8 {
        self.0
    }

    pub fn category(self) -> u8 {
        self.bits() & Self::CATEGORY_BITS
    }

    /// Whether any motion may write this bone. The sampler branch tests nothing itself (`test al,al` at
    /// `FFXiMain.dll retail-2026-09` RVA 0x1A5A3 is the merge gate); the category decides whether a bone
    /// has a channel to be written — the policy pass dispatches on it (`and eax,0x3f`, RVA 0x19AA3) and a
    /// bone with none reaches no handler.
    pub fn writable(self) -> bool {
        self.category() != 0
    }

    pub fn accepts_blend_layers(self) -> bool {
        self.bits() & Self::ACCEPTS_BLEND_LAYERS != 0
    }

    /// Clear bit 7: the bone keeps whatever its base layer produced through any crossfade.
    #[must_use]
    pub fn without_blend_layers(self) -> Self {
        Self(self.bits() & !Self::ACCEPTS_BLEND_LAYERS)
    }
}

/// Every bone's record for one frame — retail's pose scratch (`FFXiMain.dll retail-2026-09` array `.data
/// 0x1045F030`, stride `0x34`). The
/// queue resets all records to defaults each frame and then lets each active motion layer overwrite the
/// bones it keys; that reset pass is what pins the layout: quaternion at +0x00, translation at +0x10,
/// scale at +0x1C (RVA 0x1A463..0x1A4B5, writing defaults from the globals `[0x10456d2c]`..`[0x10456d38]`
/// and `[0x10456d3c]`, and scale from `.rdata 0x1035109C`).
///
/// A bone no layer keys this frame therefore holds **nothing**: the reset pass runs again next frame and
/// whatever is left standing is a live layer's sample or the skeleton's own default (`update_joint` builds
/// that bone's transform from its bind data when no record exists). Carrying an unkeyed bone across frames
/// instead makes the pose depend on which motion happened to write it last — an upper body twisted by one
/// finished battle clip and never written again stays twisted forever.
#[derive(Default)]
pub struct BonePoseScratch {
    records: Vec<Option<KeyFrameTransform>>,
}

impl BonePoseScratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Grow or shrink the record list to a skeleton's bone count. A different count is a different
    /// skeleton, so carried records do not survive it.
    pub fn set_bone_count(&mut self, bones: usize) {
        if self.records.len() != bones {
            self.records.clear();
            self.records.resize(bones, None);
        }
    }

    pub fn bone_count(&self) -> usize {
        self.records.len()
    }

    /// retail's reset pass (`FFXiMain.dll retail-2026-09` RVA 0x1A463..0x1A4B5): size the record list and
    /// clear every record to nothing, which is what makes each frame read from the live layers rather than
    /// from history.
    pub fn begin_frame(&mut self, bones: usize) {
        self.set_bone_count(bones);
        self.clear();
    }

    /// This frame's record. `None` means no live layer keyed the bone, so the caller falls back to the
    /// skeleton default.
    pub fn get(&self, bone: usize) -> Option<KeyFrameTransform> {
        self.records.get(bone).copied().flatten()
    }

    /// Drop every carried record (pose-state reset or model swap).
    pub fn clear(&mut self) {
        for record in self.records.iter_mut() {
            *record = None;
        }
    }

    /// Record what this frame sampled for one bone, including the empty case: an unkeyed bone is cleared,
    /// not inherited.
    fn write(&mut self, bone: usize, sampled: Option<KeyFrameTransform>) {
        if let Some(slot) = self.records.get_mut(bone) {
            *slot = sampled;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoopParams {
    pub loop_duration: Option<f32>,

    pub num_loops: Option<u32>,
    pub low_priority: bool,
}

impl LoopParams {
    pub fn low_priority_loop() -> Self {
        LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: true,
        }
    }
}

#[derive(Clone)]
pub struct TransitionParams {
    pub transition_in_time: f32,
    pub transition_out_time: f32,
    pub eager_transition_out: bool,
    /// Start the clip in step with the one it replaces in its slot ([`in_step_start`]) instead of on its
    /// first frame. A request without parameters of its own takes this from the clip it replaces, the way
    /// it takes that clip's out time.
    pub in_step: bool,
    /// The crossfade turns toward the target rather than the short way round where the short way would swing
    /// a bone behind both poses ([`HeldArc::set_out`]): set while the actor is locked on, facing its target.
    pub turn_toward_target: bool,
}

impl Default for TransitionParams {
    fn default() -> Self {
        TransitionParams {
            transition_in_time: 7.5,
            transition_out_time: 7.5,
            eager_transition_out: false,
            in_step: false,
            turn_toward_target: false,
        }
    }
}

/// An outgoing clip shorter than this many frames hands no step on (`FFXiMain.dll retail-2026-09`
/// `.rdata` RVA 0x32A22C, read at RVA 0x1AE87).
const IN_STEP_MIN_SPAN: f32 = 0.001;

/// Where a clip requested in step starts: the outgoing clip's key position plus the keys the incoming clip
/// plays over the blend, scaled by the ratio of the two clips' spans (frame count over key rate) and
/// wrapped into the incoming span, with the result read as a playhead. That is the arithmetic retail's
/// motion queue runs when a request that asks for it joins a slot already holding a motion (`FFXiMain.dll
/// retail-2026-09` RVA 0x1AE01..0x1AF18 in the queue add at RVA 0x1AD00, taken over the RVA 0xCD650
/// branch), and every idle, walk, run and side step request asks, a jump aside (RVA 0xC861B..0xC8686). It
/// mixes key positions with frame spans exactly as retail does, so clips whose key rate is not one land
/// where retail puts them, not on the matching fraction of the cycle.
pub fn in_step_start(
    outgoing: &SkeletonAnimationContext,
    incoming: &SkeletonAnimation,
    blend_frames: f32,
) -> f32 {
    let span = |clip: &SkeletonAnimation| clip.num_frames as f32 / clip.key_frame_duration;
    let (from, to) = (span(&outgoing.animation), span(incoming));
    let spans_usable = from > IN_STEP_MIN_SPAN && to > 0.0 && to.is_finite();
    if !spans_usable {
        return 0.0;
    }
    let keys = outgoing.current_frame * outgoing.animation.key_frame_duration
        + incoming.key_frame_duration * blend_frames;
    (keys * to / from).rem_euclid(to)
}

#[derive(Clone)]
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

    /// A clip with a loop count that has played them all, which retail's queue reads off an element whose
    /// count has run out (`FFXiMain.dll retail-2026-09` RVA 0x1A890).
    pub fn is_finished_one_shot(&self) -> bool {
        self.loop_params.num_loops.is_some() && self.completed
    }

    fn apply_loop_bounds(&mut self) -> f32 {
        let max_loops = self.loop_params.num_loops.unwrap_or(0);
        let length = self.animation.length_in_frames();

        // A playhead landing exactly on the end frame has consumed its pass. Counting that as
        // `>` only left a one-shot sitting at `current_frame == length` with `completed` false,
        // and since a selection change asks for its registration once, the clip that was waiting
        // behind it never arrived: the finished pose stayed in the slot forever.
        while length > 0.0 && self.current_frame >= length {
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

pub struct AnimationSnapshot {
    joint_snapshots: HashMap<usize, KeyFrameTransform>,
}

impl AnimationSnapshot {
    pub fn from_context(ctx: &SkeletonAnimationContext) -> Self {
        let mut joint_snapshots = HashMap::new();
        for &joint in ctx.animation.key_frame_sets.keys() {
            if let Some(t) = ctx.animation.get_joint_transform(joint, ctx.current_frame) {
                joint_snapshots.insert(joint as usize, t);
            }
        }
        AnimationSnapshot { joint_snapshots }
    }

    pub fn from_transition(transition: &AnimationTransition) -> Self {
        let mut joint_snapshots = HashMap::new();
        let joints: std::collections::HashSet<usize> = transition
            .previous
            .joint_keys()
            .into_iter()
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

/// The outgoing side of a crossfade. A layer that stays registered keeps
/// stepping under the blend, because retail samples every active motion into
/// the pose scratch each tick and merges live poses by weight (`FFXiMain.dll
/// retail-2026-09`: queue sampler RVA 0x1B230 per slot, UpdateAllChannels RVA
/// 0x1A420). Only retargeting *during* a blend freezes the composite that was
/// on screen — no single live layer matches it.
pub enum PreviousSide {
    Live(SkeletonAnimationContext),
    Frozen(AnimationSnapshot),
}

impl PreviousSide {
    fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        match self {
            PreviousSide::Live(ctx) => ctx.get_joint_transform(joint),
            PreviousSide::Frozen(snapshot) => snapshot.get_joint_transform(joint),
        }
    }

    fn advance(&mut self, elapsed_frames: f32) {
        if let PreviousSide::Live(ctx) = self {
            ctx.advance(elapsed_frames);
        }
    }

    fn joint_keys(&self) -> Vec<usize> {
        match self {
            PreviousSide::Live(ctx) => ctx
                .animation
                .key_frame_sets
                .keys()
                .map(|&k| k as usize)
                .collect(),
            PreviousSide::Frozen(snapshot) => snapshot.joint_snapshots.keys().copied().collect(),
        }
    }

    /// Whether the motion on this side is a one-shot that has played out; a still holds no motion.
    fn holds_finished_one_shot(&self) -> bool {
        match self {
            PreviousSide::Live(ctx) => ctx.is_finished_one_shot(),
            PreviousSide::Frozen(_) => false,
        }
    }
}

pub struct AnimationTransition {
    pub previous: PreviousSide,
    pub next: SkeletonAnimationContext,
    pub transition_duration: f32,
    progress: f32,
    /// Per bone, the pair this merge stood on at its last tick; empty until its first.
    arcs: Vec<Option<HeldArc>>,
    /// Whether a bone's merge sets out toward the target ([`TransitionParams::turn_toward_target`]).
    toward_target: bool,
}

impl AnimationTransition {
    pub fn new(
        previous: PreviousSide,
        next: SkeletonAnimationContext,
        transition_duration: f32,
        toward_target: bool,
    ) -> Self {
        AnimationTransition {
            previous,
            next,
            transition_duration,
            progress: 0.0,
            arcs: Vec::new(),
            toward_target,
        }
    }

    /// Every tick under the blend steps both layers and the in-flight request
    /// — gait clips keep running through a crossfade rather than pausing at
    /// their entry frame (`FFXiMain.dll retail-2026-09` sampler RVA 0x1B230).
    pub fn update(&mut self, elapsed_frames: f32) -> bool {
        self.previous.advance(elapsed_frames);
        self.next.advance(elapsed_frames);
        self.progress += elapsed_frames;
        self.hold_arcs();
        self.is_complete()
    }

    /// Record the pair each merged bone stands on at the playheads this tick left.
    fn hold_arcs(&mut self) {
        for &joint in self.next.animation.key_frame_sets.keys() {
            let joint = joint as usize;
            let (Some(outgoing), Some(incoming)) = (
                self.previous.get_joint_transform(joint),
                self.next.get_joint_transform(joint),
            ) else {
                continue;
            };
            let held = self.arcs.get(joint).copied().flatten();
            let arc = arc_for(
                held,
                outgoing.rotation,
                incoming.rotation,
                self.toward_target,
            );
            if self.arcs.len() <= joint {
                self.arcs.resize(joint + 1, None);
            }
            self.arcs[joint] = Some(arc);
        }
    }

    /// Whether any motion in this blend is a one-shot that has played out.
    fn holds_finished_one_shot(&self) -> bool {
        self.next.is_finished_one_shot() || self.previous.holds_finished_one_shot()
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= self.transition_duration
    }

    /// One weighted sum of the outgoing and incoming bone records, with no third pose in it (`FFXiMain.dll
    /// retail-2026-09` RVA 0x33220), taken along the pair the merge holds for the bone ([`HeldArc`]).
    ///
    /// A clip that does not key a bone writes nothing for it, so a side with no key is not a bind-pose key:
    /// whichever side keys the bone owns it outright (RVA 0x1B230 samples a slot and writes only the bones its
    /// motion keys; nothing blends a bone a layer never wrote). Blending against a stand-in instead walked every
    /// unkeyed bone to bind over the fade, which is the floating weapon: a battle turn-in-place clip keys no
    /// weapon joint, so each turn faded the weapon out of the hand.
    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        let t = self.progress / self.transition_duration;
        match (
            self.previous.get_joint_transform(joint),
            self.next.get_joint_transform(joint),
        ) {
            (Some(outgoing), Some(incoming)) => {
                let held = self.arcs.get(joint).copied().flatten();
                let arc = arc_for(
                    held,
                    outgoing.rotation,
                    incoming.rotation,
                    self.toward_target,
                );
                Some(KeyFrameTransform {
                    rotation: arc.rotation(t),
                    translation: lerp3(outgoing.translation, incoming.translation, t),
                    scale: lerp3(outgoing.scale, incoming.scale, t),
                })
            }
            (only, None) | (None, only) => only,
        }
    }
}

/// The pair a merge takes this tick: the one it held, kept continuous, or a fresh one.
fn arc_for(
    held: Option<HeldArc>,
    outgoing: [f32; 4],
    incoming: [f32; 4],
    toward_target: bool,
) -> HeldArc {
    match held {
        Some(arc) => arc.held(outgoing, incoming),
        None => HeldArc::set_out(outgoing, incoming, toward_target),
    }
}

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
        if matches!(
            self.transition.as_mut().map(|t| t.update(elapsed_frames)),
            Some(true)
        ) {
            self.transition = None;
        }
        // Stepping `current_animation` on every tick — including the one where
        // a blend completes — keeps its playhead on the frame the blend just
        // sampled, so the handoff cannot jump.
        if let Some(ctx) = self.current_animation.as_mut() {
            ctx.advance(elapsed_frames);
        }
    }

    /// Whether any motion in this slot is a one-shot that has played out (`FFXiMain.dll retail-2026-09` RVA
    /// 0x1B360 asks each element of a slot).
    pub fn holds_finished_one_shot(&self) -> bool {
        self.current_animation
            .as_ref()
            .is_some_and(SkeletonAnimationContext::is_finished_one_shot)
            || self
                .transition
                .as_ref()
                .is_some_and(AnimationTransition::holds_finished_one_shot)
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
            self.transition = None;
            self.current_animation = Some(ctx);
            return;
        }

        let current = self.current_animation.as_ref().unwrap();

        if same_animation(&current.animation, &ctx.animation) && current.loop_params.low_priority {
            return;
        }

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
        let in_step = match transition_params {
            Some(tp) => tp.in_step,
            None => current
                .transition_params
                .as_ref()
                .is_some_and(|t| t.in_step),
        };
        let mut ctx = ctx;
        if in_step {
            ctx.current_frame = in_step_start(current, &ctx.animation, transition_duration);
        }

        if self.animation_slot != 5 {
            let previous = match &self.transition {
                Some(t) => PreviousSide::Frozen(AnimationSnapshot::from_transition(t)),
                None => PreviousSide::Live(current.clone()),
            };

            self.transition = Some(AnimationTransition::new(
                previous,
                fresh_copy(&ctx),
                transition_duration,
                transition_params.is_some_and(|tp| tp.turn_toward_target),
            ));
        }

        self.current_animation = Some(ctx);
    }

    /// A handover that settles instead of playing out. The outgoing side is snapshotted at the tick of
    /// registration and never advances again - a stop asks for rest, so nothing may swing further than
    /// it already was (see `register_idle_animation_matched`). The incoming clip enters at `start_frame`,
    /// chosen by the caller to match that frozen pose, and rises over `fade_frames`. Slot 5 behaves as in
    /// [`Self::set_next_animation`]: replaced outright, no blend.
    pub fn set_settled_animation(
        &mut self,
        animation: SkeletonAnimation,
        loop_params: LoopParams,
        start_frame: f32,
        fade_frames: f32,
    ) {
        let mut ctx = SkeletonAnimationContext::new(animation, loop_params, None);
        let length = ctx.animation.length_in_frames();
        if length > 0.0 {
            ctx.current_frame = start_frame.rem_euclid(length);
        }
        if self.current_animation.is_none() || fade_frames <= 0.0 || self.animation_slot == 5 {
            self.transition = None;
            self.current_animation = Some(ctx);
            return;
        }
        let previous = match &self.transition {
            // Interrupting a live blend: freeze the composite on screen, exactly as a retarget during a
            // blend does in `set_next_animation`.
            Some(t) => PreviousSide::Frozen(AnimationSnapshot::from_transition(t)),
            None => PreviousSide::Frozen(AnimationSnapshot::from_context(
                self.current_animation.as_ref().unwrap(),
            )),
        };
        self.transition = Some(AnimationTransition::new(
            previous,
            fresh_copy(&ctx),
            fade_frames,
            false,
        ));
        self.current_animation = Some(ctx);
    }

    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        self.sample(joint, true)
    }

    /// Sample this layer for one bone, refusing the crossfade when the bone's mask has cleared bit 7.
    /// A refused blend leaves the bone on whichever side was already in the slot: retail runs a blend
    /// as an extra layer over the base sample and skips that merge entirely when bit 7 is clear
    /// (`test al,al` / `jns` at `FFXiMain.dll retail-2026-09` RVA 0x1A5A3), and kuluu's slot keeps its
    /// outgoing side as `previous` while the blend runs.
    fn sample(&self, joint: usize, blend_allowed: bool) -> Option<KeyFrameTransform> {
        match &self.transition {
            Some(t) if blend_allowed => t.get_joint_transform(joint),
            Some(t) => t.previous.get_joint_transform(joint),
            None => self
                .current_animation
                .as_ref()
                .and_then(|c| c.get_joint_transform(joint)),
        }
    }
}

fn same_animation(a: &SkeletonAnimation, b: &SkeletonAnimation) -> bool {
    a.id == b.id
}

/// A copy of a freshly requested clip with its counters at zero, starting where the request put its playhead.
fn fresh_copy(ctx: &SkeletonAnimationContext) -> SkeletonAnimationContext {
    let mut copy = SkeletonAnimationContext::new(
        ctx.animation.clone(),
        ctx.loop_params,
        ctx.transition_params.clone(),
    );
    copy.current_frame = ctx.current_frame;
    copy
}

#[derive(Default)]
pub struct SkeletonAnimationCoordinator {
    pub animations: [Option<SkeletonAnimator>; 8],
    /// One mask byte per bone (`FFXiMain.dll retail-2026-09` array `.data 0x1045F028`, the law in
    /// [`BoneMotionMask`]).
    masks: Vec<BoneMotionMask>,
}

impl SkeletonAnimationCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Size the mask array to a skeleton. Growing fills with [`BoneMotionMask::DEFAULT`] (everything
    /// allowed), shrinking drops the tail.
    pub fn set_bone_count(&mut self, bones: usize) {
        if self.masks.len() < bones {
            self.masks.resize(bones, BoneMotionMask::DEFAULT);
        } else if self.masks.len() > bones {
            self.masks.truncate(bones);
        }
    }

    pub fn mask_at(&self, bone: usize) -> BoneMotionMask {
        self.masks
            .get(bone)
            .copied()
            .unwrap_or(BoneMotionMask::DEFAULT)
    }

    /// Set one bone's mask, growing the array with defaults as needed. Callers use this to take bones
    /// away from a layer (clearing the category) or to freeze them against crossfades (clearing bit 7).
    pub fn set_mask(&mut self, bone: usize, mask: BoneMotionMask) {
        if bone >= self.masks.len() {
            self.masks.resize(bone + 1, BoneMotionMask::DEFAULT);
        }
        self.masks[bone] = mask;
    }

    /// Reset every bone to the default (new model instance).
    pub fn reset_masks(&mut self) {
        self.masks.clear();
    }

    pub fn update(&mut self, elapsed_frames: f32) {
        for slot in self.animations.iter_mut().flatten() {
            slot.update(elapsed_frames);
        }
    }

    /// Installs the request when that slot's current layer will hand over, and says whether it did: a
    /// caller that assumes success can strand the slot on whoever refused.
    pub fn register_animation(
        &mut self,
        animation: SkeletonAnimation,
        loop_params: LoopParams,
        transition_params: Option<TransitionParams>,
        override_condition: impl Fn(&SkeletonAnimator) -> bool,
    ) -> bool {
        let slot = animation.id.final_digit().unwrap_or(0) as usize;
        let slot = slot.min(7);
        let animator = self.get_or_put(slot);

        if override_condition(animator) {
            let ctx =
                SkeletonAnimationContext::new(animation, loop_params, transition_params.clone());
            animator.set_next_animation(ctx, transition_params.as_ref());
            true
        } else {
            false
        }
    }

    pub fn register_idle_animation(
        &mut self,
        animation: SkeletonAnimation,
        require_transition_out: bool,
    ) -> bool {
        self.register_animation(
            animation,
            LoopParams::low_priority_loop(),
            None,
            |animator| ready_for_transition_out(animator, require_transition_out),
        )
    }

    /// The idle registration that hands the slot over at once even though the
    /// current clip is still mid-loop: the action driving that clip just
    /// ended, and retail's kill drops the sequence's animation instance, so
    /// the pinned end frame must not outlive it. The crossfade runs the
    /// current clip's own transition-out window.
    pub fn register_idle_animation_eager(&mut self, animation: SkeletonAnimation) -> bool {
        self.register_animation(animation, LoopParams::low_priority_loop(), None, |_| true)
    }

    /// The idle registration for a stop: the outgoing pose is frozen where it stands and the incoming
    /// clip takes its playhead from the key frame closest to that pose. A held gait is still playing at
    /// the tick the movement ends, and letting it run under the crossfade winds limbs back past what
    /// they were holding before landing in idle - measured on Hume M at 15-30 deg of fresh swing after
    /// release against holds of 20 deg or less (`stopping_a_held_movement_never_winds_the_arms_up_
    /// after_release`, kuluu-render). A stop settles: it matches key frames and smooths, and plays no
    /// animation further.
    pub fn register_idle_animation_matched(
        &mut self,
        animation: SkeletonAnimation,
        start_frame: f32,
        fade_frames: f32,
    ) -> bool {
        let slot = animation.id.final_digit().unwrap_or(0) as usize;
        let slot = slot.min(7);
        let animator = self.get_or_put(slot);
        if !ready_for_transition_out(animator, true) {
            return false;
        }
        animator.set_settled_animation(
            animation,
            LoopParams::low_priority_loop(),
            start_frame,
            fade_frames,
        );
        true
    }

    /// This frame's transform for one bone. Ownership is per bone, never per slot: retail samples every
    /// active layer into the shared pose scratch (`.data 0x1045F030`) with each sample overwriting only
    /// the bones its clip keys, walking layer indices descending (`mov edi,4` … `dec edi` / `jge`,
    /// `FFXiMain.dll retail-2026-09` RVA 0x1A4E8..0x1A530), so the owner is the lowest-indexed active
    /// layer that keys the bone. A whole-slot blend has no counterpart there: taking a bone away from a
    /// layer means clearing that bone's channel category, not fading a slot.
    ///
    /// The mask gates this pass on two counts — a bone with no channel category is written by nothing
    /// (`and eax,0x3f`, RVA 0x19AA3), and a bone with bit 7 cleared samples without its crossfade merge
    /// (`test al,al` / `jns`, RVA 0x1A5A3).
    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        self.sample_joint(joint)
    }

    pub fn sample_joint(&self, joint: usize) -> Option<KeyFrameTransform> {
        let mask = self.mask_at(joint);
        if !mask.writable() {
            return None;
        }
        let blend_allowed = mask.accepts_blend_layers();
        for animator in self.animations.iter().flatten() {
            if let Some(t) = animator.sample(joint, blend_allowed) {
                return Some(t);
            }
        }
        None
    }

    /// A frame of the pose pass: retail's reset pass first (`FFXiMain.dll retail-2026-09` RVA
    /// 0x1A463..0x1A4B5 clears every bone record before any sampling), then each bone sampled through the
    /// ownership law. A bone no live layer keys ends the frame with no record; it is not inherited.
    pub fn sample_pose(&mut self, bones: usize, scratch: &mut BonePoseScratch) {
        self.set_bone_count(bones);
        scratch.begin_frame(bones);
        for bone in 0..scratch.bone_count() {
            let sampled = self.sample_joint(bone);
            scratch.write(bone, sampled);
        }
    }

    pub fn is_transitioning(&self) -> bool {
        self.animations
            .iter()
            .flatten()
            .any(|a| a.transition.is_some())
    }

    /// Drop every layer and mask: the next frame starts from bind again, so carried records must go with
    /// them (`BonePoseScratch::clear`).
    pub fn clear(&mut self) {
        for slot in self.animations.iter_mut() {
            *slot = None;
        }
        self.masks.clear();
    }

    pub fn occupied_slots(&self) -> u8 {
        let mut mask = 0u8;
        for (i, slot) in self.animations.iter().enumerate() {
            if slot.is_some() {
                mask |= 1 << i;
            }
        }
        mask
    }

    pub fn clear_slot(&mut self, slot: usize) {
        if let Some(s) = self.animations.get_mut(slot) {
            *s = None;
        }
    }

    pub fn holds_finished_one_shot(&self, slot: usize) -> bool {
        self.animations
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(SkeletonAnimator::holds_finished_one_shot)
    }

    fn get_or_put(&mut self, slot: usize) -> &mut SkeletonAnimator {
        if self.animations[slot].is_none() {
            self.animations[slot] = Some(SkeletonAnimator::new(slot));
        }
        self.animations[slot].as_mut().unwrap()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::datid::DatId;
    use ffxi_dat::skel_anim::SkeletonAnimation;
    use std::collections::HashMap;

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
            anim("idl0", 5, 1.0),
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            None,
        );

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
            anim("idl0", 5, 1.0),
            LoopParams {
                loop_duration: None,
                num_loops: Some(1),
                low_priority: false,
            },
            None,
        );
        assert!(!ctx.is_done_looping());
        ctx.advance(5.0);
        assert!(ctx.is_done_looping());
        assert_eq!(ctx.current_frame, 4.0);
    }

    /// A one-shot whose playhead lands exactly on its end frame has finished. `is_done_looping` gates
    /// whether the next clip may take the slot, and a selection change asks for that handoff once —
    /// an incomplete boundary therefore strands the finished pose in the slot permanently.
    #[test]
    fn landing_exactly_on_the_end_frame_completes_a_one_shot() {
        let animation = anim("otd0", 5, 1.0);
        let length = animation.length_in_frames();
        let mut ctx = SkeletonAnimationContext::new(
            animation,
            LoopParams {
                loop_duration: None,
                num_loops: Some(1),
                low_priority: false,
            },
            None,
        );

        assert!(!ctx.is_done_looping());
        ctx.advance(length / 2.0);
        ctx.advance(length / 2.0);

        assert!(
            ctx.is_done_looping(),
            "frame {} of {length}",
            ctx.current_frame
        );
        assert_eq!(ctx.current_frame, length);
    }

    /// Looping clips are unchanged by that rule: on a loop the end frame and the first are one pose.
    #[test]
    fn a_looping_clip_wraps_at_the_end_frame_rather_than_resting_on_it() {
        let animation = anim("idl0", 5, 1.0);
        let length = animation.length_in_frames();
        let mut ctx = SkeletonAnimationContext::new(
            animation,
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            None,
        );

        ctx.advance(length);

        assert_eq!(ctx.current_frame, 0.0);
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

        let t0 = coord.get_joint_transform(0).unwrap();
        assert!((t0.translation[0] - 0.0).abs() < 1e-4);

        coord.update(0.5);
        let t1 = coord.get_joint_transform(0).unwrap();
        assert!(
            (t1.translation[0] - 5.0).abs() < 1e-3,
            "got {}",
            t1.translation[0]
        );
    }

    /// Retail walks base layer indices descending and lets each sample overwrite the last (`mov edi,4` …
    /// `dec edi` / `jge`, `FFXiMain.dll retail-2026-09` RVA 0x1A4E8..0x1A530), so the owner of a bone is
    /// the lowest-indexed active layer that keys it — there is no slot-into-slot fade to work around.
    #[test]
    fn the_lowest_indexed_layer_that_keys_a_bone_owns_it() {
        let mut coord = SkeletonAnimationCoordinator::new();
        coord.register_animation(
            authored("aaa0", 0, 10.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );
        coord.register_animation(
            authored("bbb3", 0, 100.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );

        let t = coord.get_joint_transform(0).unwrap();
        assert!(
            (t.translation[0] - 10.0).abs() < 1e-4,
            "slot 3 took a bone slot 0 owns: got {}",
            t.translation[0]
        );
    }

    #[test]
    fn a_bone_keyed_only_by_the_higher_layer_still_comes_through() {
        let mut coord = SkeletonAnimationCoordinator::new();
        coord.register_animation(
            authored("aaa0", 0, 10.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );
        coord.register_animation(
            authored("bbb3", 7, 100.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );

        let t = coord.get_joint_transform(7).unwrap();
        assert!(
            (t.translation[0] - 100.0).abs() < 1e-4,
            "got {}",
            t.translation[0]
        );
    }

    /// A bone with no channel category has nothing to sample: the policy pass dispatches on that field
    /// (`and eax,0x3f`, `FFXiMain.dll retail-2026-09` RVA 0x19AA3) and a bone below every handler is never
    /// written. Clearing it is how kuluu takes a bone away from the clips that key it.
    #[test]
    fn a_mask_without_a_channel_category_is_written_by_nothing() {
        let mut coord = SkeletonAnimationCoordinator::new();
        coord.set_bone_count(8);
        coord.register_animation(
            authored("aaa0", 3, 10.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );

        assert!(coord.get_joint_transform(3).is_some());

        coord.set_mask(3, BoneMotionMask::BLOCKED);
        assert!(coord.get_joint_transform(3).is_none());
        assert!(
            coord.get_joint_transform(4).is_none(),
            "masking one bone must not touch the others' sampling"
        );
    }

    /// A bone whose mask cleared bit 7 takes no merge, so it stands on its outgoing side for the whole
    /// crossfade while its neighbours blend.
    #[test]
    fn a_bone_refusing_blend_layers_holds_its_outgoing_side() {
        let mut coord = SkeletonAnimationCoordinator::new();
        coord.set_bone_count(8);
        coord.register_animation(
            authored("idl0", 0, 10.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );
        let tp = TransitionParams {
            transition_in_time: 7.5,
            ..Default::default()
        };
        coord.register_animation(
            authored("run0", 0, 100.0),
            LoopParams::low_priority_loop(),
            Some(tp.clone()),
            |_| true,
        );
        // Part-way through the blend, so an allowed merge is measurably between its two sides.
        coord.update(2.5);
        assert!(coord.is_transitioning());

        coord.set_mask(0, BoneMotionMask::DEFAULT.without_blend_layers());
        let refused = coord.get_joint_transform(0).unwrap().translation[0];

        let animators = coord.animations[0].as_ref().unwrap();
        let outgoing = match &animators.transition.as_ref().unwrap().previous {
            PreviousSide::Live(p) => p.get_joint_transform(0).unwrap().translation[0],
            PreviousSide::Frozen(s) => s.get_joint_transform(0).unwrap().translation[0],
        };
        assert!(
            (refused - outgoing).abs() < 1e-4,
            "blend not refused at the bone: {refused} vs outgoing {outgoing}"
        );

        coord.set_mask(0, BoneMotionMask::DEFAULT);
        let blended = coord.get_joint_transform(0).unwrap().translation[0];
        assert!(
            (blended - refused).abs() > 1e-4,
            "the same bone should blend once bit 7 is set again: {blended} vs {refused}"
        );
    }

    /// Nothing inherits: once no live layer keys a bone its record is gone on the next frame, so a pose can
    /// never keep a rotation left behind by a motion that has finished. retail runs this reset pass before
    /// every sample (`FFXiMain.dll retail-2026-09` RVA 0x1A463..0x1A4B5).
    #[test]
    fn an_unkeyed_bone_is_reset_the_next_frame() {
        let mut coord = SkeletonAnimationCoordinator::new();
        let mut scratch = BonePoseScratch::new();
        coord.register_animation(
            authored("aaa0", 5, 42.0),
            LoopParams::low_priority_loop(),
            None,
            |_| true,
        );

        coord.sample_pose(9, &mut scratch);
        assert!((scratch.get(5).unwrap().translation[0] - 42.0).abs() < 1e-4);

        coord.clear_slot(0);
        coord.sample_pose(9, &mut scratch);
        assert!(
            scratch.get(5).is_none(),
            "bone 5 outlived the layer that keyed it"
        );
    }

    /// A crossfade out of a clip into one that never keys the bone leaves the bone where the outgoing
    /// clip has it through the whole fade — the unkeyed side is not a bind-pose key, and fading against one
    /// walked the weapon out of the hand mid-turn. When the fade finishes and nothing keys the bone any
    /// more, its record goes with them.
    #[test]
    fn a_bone_the_incoming_clip_never_keys_holds_the_outgoing_pose_through_the_fade() {
        let mut coord = SkeletonAnimationCoordinator::new();
        let mut scratch = BonePoseScratch::new();
        let loops = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: false,
        };
        let tp = TransitionParams {
            transition_in_time: 8.0,
            transition_out_time: 8.0,
            ..Default::default()
        };
        // btl0 keys bone 5 (the weapon) at 42; the turn clip keys only bone 0.
        coord.register_animation(authored("btl0", 5, 42.0), loops, Some(tp.clone()), |_| true);
        coord.update(1.0);
        coord.sample_pose(9, &mut scratch);
        assert!((scratch.get(5).unwrap().translation[0] - 42.0).abs() < 1e-4);

        coord.register_animation(authored("ind0", 0, 7.0), loops, Some(tp), |_| true);
        for _ in 0..4 {
            coord.update(1.0);
            coord.sample_pose(9, &mut scratch);
            assert!(coord.is_transitioning(), "the fade is still running");
            assert!(
                (scratch.get(5).unwrap().translation[0] - 42.0).abs() < 1e-4,
                "mid-fade the weapon bone must stay where btl0 left it, got {}",
                scratch.get(5).unwrap().translation[0]
            );
        }
        for _ in 0..8 {
            coord.update(1.0);
            coord.sample_pose(9, &mut scratch);
        }
        assert!(!coord.is_transitioning());
        assert!(
            scratch.get(5).is_none(),
            "the weapon bone outlived every layer that keyed it"
        );
    }

    /// The other direction: when only the incoming side keys a bone, that side owns the bone outright —
    /// there is no ramp in from wherever the frame before left it and no blend against bind.
    #[test]
    fn a_bone_only_the_incoming_side_keys_is_written_by_it_at_full_weight() {
        let mut coord = SkeletonAnimationCoordinator::new();
        let mut scratch = BonePoseScratch::new();
        let loops = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: false,
        };
        let tp = TransitionParams {
            transition_in_time: 10.0,
            transition_out_time: 10.0,
            ..Default::default()
        };
        coord.register_animation(authored("btl0", 5, 42.0), loops, Some(tp.clone()), |_| true);
        coord.update(1.0);
        coord.sample_pose(9, &mut scratch);
        coord.register_animation(authored("ind0", 0, 7.0), loops, Some(tp.clone()), |_| true);
        for _ in 0..12 {
            coord.update(1.0);
            coord.sample_pose(9, &mut scratch);
        }
        // Bone 5 has no live key; now a clip keys it at 2 — mid-fade is that sample, not a midpoint.
        coord.register_animation(authored("btl0", 5, 2.0), loops, Some(tp), |_| true);
        coord.update(1.0);
        let sampled = coord.get_joint_transform(5).unwrap().translation[0];
        assert!(
            (sampled - 2.0).abs() < 1e-3,
            "the side that keys the bone owns it, got {sampled}"
        );
    }

    /// One frame of one bone, at a chosen value, so precedence assertions read as authored numbers.
    fn authored(id: &str, joint: u32, x: f32) -> SkeletonAnimation {
        let mut clip = anim(id, 1, 1.0);
        let mut frames = clip.key_frame_sets.remove(&0).expect("anim keys joint 0");
        frames[0].translation[0] = x;
        clip.key_frame_sets.insert(joint, frames);
        clip
    }

    #[test]
    fn same_low_priority_idle_not_retriggered() {
        let mut animator = SkeletonAnimator::new(0);
        let a = anim("idl0", 3, 1.0);
        animator.set_next_animation(
            SkeletonAnimationContext::new(a.clone(), LoopParams::low_priority_loop(), None),
            None,
        );

        animator.update(1.0);
        let frame_before = animator.current_animation.as_ref().unwrap().current_frame;

        animator.set_next_animation(
            SkeletonAnimationContext::new(a.clone(), LoopParams::low_priority_loop(), None),
            None,
        );
        let frame_after = animator.current_animation.as_ref().unwrap().current_frame;
        assert_eq!(frame_before, frame_after);
        assert!(animator.transition.is_none());
    }

    /// Retail's queue samples every active layer each tick and crossfades
    /// live poses (`FFXiMain.dll retail-2026-09` sampler RVA 0x1B230), so a
    /// gait clip must keep stepping underneath the blend instead of pausing
    /// at its entry frame — and the completion handoff must land on the same
    /// frame the blend was sampling.
    #[test]
    fn a_crossfade_steps_both_layers_under_the_blend() {
        let mut animator = SkeletonAnimator::new(0);
        animator.set_next_animation(
            SkeletonAnimationContext::new(
                anim("idl0", 13, 1.0),
                LoopParams::low_priority_loop(),
                None,
            ),
            None,
        );
        animator.update(2.0);

        let tp = TransitionParams {
            transition_in_time: 7.5,
            ..Default::default()
        };
        animator.set_next_animation(
            SkeletonAnimationContext::new(
                anim("run0", 13, 1.0),
                LoopParams {
                    loop_duration: None,
                    num_loops: None,
                    low_priority: false,
                },
                Some(tp.clone()),
            ),
            Some(&tp),
        );

        animator.update(1.0);
        let t = animator.transition.as_ref().unwrap();
        assert!(matches!(&t.previous, PreviousSide::Live(p) if p.current_frame == 3.0));
        assert_eq!(t.next.current_frame, 1.0);

        // Helper clips put joint x at frame*10: one tick in, prev sits at 30
        // and the incoming clip at 10 — both moving.
        let visible = animator.get_joint_transform(0).unwrap().translation[0];
        let expected = 30.0 + (10.0 - 30.0) * (1.0 / 7.5);
        assert!((visible - expected).abs() < 1e-3, "got {visible}");

        animator.update(7.0);
        assert!(animator.transition.is_none());
        let handed_off = animator.current_animation.as_ref().unwrap().current_frame;
        assert_eq!(handed_off, 8.0);
    }

    #[test]
    fn zero_in_replacement_renders_new_pose_during_an_outgoing_blend() {
        const RAISED_TRANSLATION: f32 = 100.0;
        let mut raised = anim("mw10", 3, 1.0);
        for frame in raised.key_frame_sets.get_mut(&0).unwrap() {
            frame.translation[0] = RAISED_TRANSLATION;
        }
        let looping = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: false,
        };
        let mut animator = SkeletonAnimator::new(0);
        animator.set_next_animation(
            SkeletonAnimationContext::new(raised.clone(), looping, None),
            None,
        );
        animator.set_next_animation(
            SkeletonAnimationContext::new(anim("idl0", 3, 1.0), looping, None),
            None,
        );
        animator.update(1.0);
        assert!(animator.get_joint_transform(0).unwrap().translation[0] < RAISED_TRANSLATION);

        raised.id = DatId::from_str("mw20");
        let immediate = TransitionParams {
            transition_in_time: 0.0,
            ..Default::default()
        };
        animator.set_next_animation(
            SkeletonAnimationContext::new(raised, looping, Some(immediate.clone())),
            Some(&immediate),
        );
        for _ in 0..3 {
            assert_eq!(
                animator.get_joint_transform(0).unwrap().translation[0],
                RAISED_TRANSLATION
            );
            animator.update(1.0);
        }
    }

    fn locomotion_blend(frames: f32) -> TransitionParams {
        TransitionParams {
            transition_in_time: frames,
            ..Default::default()
        }
    }

    fn request(animator: &mut SkeletonAnimator, clip: SkeletonAnimation, blend: Option<f32>) {
        let tp = blend.map(locomotion_blend);
        let looping = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: false,
        };
        animator.set_next_animation(
            SkeletonAnimationContext::new(clip, looping, tp.clone()),
            tp.as_ref(),
        );
    }

    fn x_of(animator: &SkeletonAnimator) -> f32 {
        animator.get_joint_transform(0).unwrap().translation[0]
    }

    /// A request that lands mid-blend crossfades from the blend as it stood on screen at that moment: the
    /// running blend is kept as a still, not stepped on under the new one.
    #[test]
    fn a_request_mid_blend_crossfades_from_the_blend_as_it_stood() {
        let mut animator = SkeletonAnimator::new(0);
        request(&mut animator, authored("aaa0", 0, 0.0), None);
        request(&mut animator, authored("bbb0", 0, 10.0), Some(8.0));
        animator.update(2.0);
        let on_screen = x_of(&animator);
        request(&mut animator, authored("ccc0", 0, 100.0), Some(12.0));
        assert!(matches!(
            animator.transition.as_ref().unwrap().previous,
            PreviousSide::Frozen(_)
        ));
        animator.update(1.0);
        let want = on_screen + (100.0 - on_screen) / 12.0;
        assert!(
            (x_of(&animator) - want).abs() < 1e-3,
            "the new blend should start from the still: got {}, want {want}",
            x_of(&animator)
        );
    }

    fn request_in_step(animator: &mut SkeletonAnimator, clip: SkeletonAnimation, blend: f32) {
        let tp = TransitionParams {
            in_step: true,
            ..locomotion_blend(blend)
        };
        let looping = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: false,
        };
        animator.set_next_animation(
            SkeletonAnimationContext::new(clip, looping, Some(tp.clone())),
            Some(&tp),
        );
    }

    fn playhead(animator: &SkeletonAnimator) -> f32 {
        animator.current_animation.as_ref().unwrap().current_frame
    }

    /// A request in step starts where the outgoing clip hands it on: its key position plus the keys the incoming
    /// clip plays over the blend, scaled from one clip's span (frame count over key rate) to the other's. The
    /// slot's clip and the blend's incoming side both start there.
    #[test]
    fn a_request_in_step_starts_where_the_outgoing_clip_hands_it_on() {
        // Outgoing: 20 keys at one per frame, six frames in. Incoming: 12 keys at half a key per frame, a span of
        // 24. Six keys plus the four played over an 8-frame blend is 10, scaled by 24/20 to 12.
        let mut animator = SkeletonAnimator::new(0);
        request(&mut animator, anim("aaa0", 20, 1.0), None);
        animator.update(6.0);
        request_in_step(&mut animator, anim("bbb0", 12, 0.5), 8.0);
        assert!((playhead(&animator) - 12.0).abs() < 1e-5);
        let incoming = &animator.transition.as_ref().unwrap().next;
        assert!((incoming.current_frame - 12.0).abs() < 1e-5);
    }

    /// Retail reads the outgoing playhead in keys against frame spans, so a key rate off one does not land on
    /// the matching fraction of the cycle; kuluu lands where retail does, wrapped into the incoming span.
    #[test]
    fn a_key_rate_off_one_lands_where_retail_does() {
        // Outgoing: 20 keys at two per frame (span 10), three frames in, so six keys. Incoming: 10 keys at one
        // per frame (span 10). Six keys plus eight over the blend is 14, which wraps to 4.
        let mut animator = SkeletonAnimator::new(0);
        request(&mut animator, anim("aaa0", 20, 2.0), None);
        animator.update(3.0);
        request_in_step(&mut animator, anim("bbb0", 10, 1.0), 8.0);
        assert!((playhead(&animator) - 4.0).abs() < 1e-5);
    }

    /// The idle a stop brings in carries no parameters of its own, so it takes the step from the clip it
    /// replaces, as it takes that clip's out time. A request out of step starts on its first frame.
    #[test]
    fn an_idle_return_takes_its_step_from_the_clip_it_replaces() {
        let mut animator = SkeletonAnimator::new(0);
        request(&mut animator, anim("idl0", 20, 1.0), None);
        // From the idle on frame 0: eight keys over the blend, so the run starts on frame 8.
        request_in_step(&mut animator, anim("run0", 20, 1.0), 8.0);
        assert!((playhead(&animator) - 8.0).abs() < 1e-5);
        animator.update(10.0);

        // The run on frame 18 hands on 18 keys plus 7.5 over its out time: 25.5, wrapped to 5.5.
        animator.set_next_animation(
            SkeletonAnimationContext::new(
                anim("idl0", 20, 1.0),
                LoopParams::low_priority_loop(),
                None,
            ),
            None,
        );
        assert!((playhead(&animator) - 5.5).abs() < 1e-4);

        request(&mut animator, anim("act0", 20, 1.0), Some(8.0));
        assert_eq!(playhead(&animator), 0.0);
    }

    /// One bone turned about y, so a test can read which way a blend swings it.
    fn turned(id: &str, deg: f32) -> SkeletonAnimation {
        let mut clip = anim(id, 1, 1.0);
        clip.key_frame_sets.get_mut(&0).unwrap()[0].rotation = yaw_quat(deg);
        clip
    }

    /// The turn a bone makes between two ticks, in degrees, whatever the sign of either quaternion.
    fn turn_between(a: [f32; 4], b: [f32; 4]) -> f32 {
        let unit = |q: [f32; 4]| q.map(|c| c / dot4(q, q).sqrt());
        (2.0 * dot4(unit(a), unit(b)).abs().min(1.0).acos()).to_degrees()
    }

    /// One bone turning about y from `from` to `to` degrees across `keys` keys, one key per frame.
    fn turning(id: &str, from: f32, to: f32, keys: usize) -> SkeletonAnimation {
        let mut clip = anim(id, keys, 1.0);
        let last = (keys - 1) as f32;
        for (k, key) in clip
            .key_frame_sets
            .get_mut(&0)
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            key.rotation = yaw_quat(from + (to - from) * k as f32 / last);
        }
        clip
    }

    /// An incoming pose that crosses the half turn from the outgoing one mid-blend keeps the blend going the way
    /// it set out. Taking the short way afresh every tick swings the bone over to the other side in one tick
    /// the moment the short way changes sides.
    #[test]
    fn a_blend_keeps_its_way_round_when_the_poses_cross_a_half_turn() {
        const FRAME: f32 = 0.5;
        const BLEND: f32 = 8.0;
        const WIDEST_TICK_DEG: f32 = 45.0;
        let mut animator = SkeletonAnimator::new(0);
        request(&mut animator, turned("aaa0", 0.0), None);
        request(&mut animator, turning("bbb0", 170.0, 200.0, 9), Some(BLEND));
        let mut last = animator.get_joint_transform(0).unwrap().rotation;
        let mut widest_tick: f32 = 0.0;
        for _ in 0..(BLEND / FRAME) as usize {
            animator.update(FRAME);
            let now = animator.get_joint_transform(0).unwrap().rotation;
            widest_tick = widest_tick.max(turn_between(last, now));
            last = now;
        }
        assert!(
            widest_tick < WIDEST_TICK_DEG,
            "the bone jumped {widest_tick} degrees in one tick"
        );
    }

    /// A crossfade between poses turned past a right angle either side, a left side step to a right one, turns
    /// through the target when it turns toward it: it sets out the opposite way to the short one, which swings the
    /// bone round behind both. Any other crossfade keeps the short way.
    #[test]
    fn a_crossfade_toward_the_target_turns_through_it_and_any_other_the_short_way() {
        const BLEND: f32 = 8.0;
        const FRONT_NEARNESS: f32 = 0.9;
        const BACK_NEARNESS: f32 = 0.1;
        let mid_blend = |toward_target: bool| {
            let mut animator = SkeletonAnimator::new(0);
            request(&mut animator, turned("mvl1", 100.0), None);
            let tp = TransitionParams {
                turn_toward_target: toward_target,
                ..locomotion_blend(BLEND)
            };
            let looping = LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            };
            animator.set_next_animation(
                SkeletonAnimationContext::new(turned("mvr1", -100.0), looping, Some(tp.clone())),
                Some(&tp),
            );
            animator.update(BLEND / 2.0);
            nearness_to_bind(animator.get_joint_transform(0).unwrap().rotation)
        };
        let toward = mid_blend(true);
        assert!(
            toward > FRONT_NEARNESS,
            "turning toward the target, mid-crossfade sits {} deg off it",
            (2.0 * toward.acos()).to_degrees()
        );
        let short = mid_blend(false);
        assert!(
            short < BACK_NEARNESS,
            "any other crossfade keeps the short way round the back: {} deg",
            (2.0 * short.acos()).to_degrees()
        );
    }

    /// A slot still blending away from a one-shot that has played out holds it until the blend is done.
    #[test]
    fn a_slot_blending_off_a_played_out_one_shot_still_holds_it() {
        let mut animator = SkeletonAnimator::new(0);
        let once = LoopParams {
            loop_duration: None,
            num_loops: Some(1),
            low_priority: false,
        };
        animator.set_next_animation(
            SkeletonAnimationContext::new(anim("atk0", 3, 1.0), once, None),
            None,
        );
        assert!(!animator.holds_finished_one_shot());
        animator.update(5.0);
        assert!(animator.holds_finished_one_shot());
        request(&mut animator, anim("idl0", 3, 1.0), Some(8.0));
        assert!(
            animator.holds_finished_one_shot(),
            "it is still under the blend"
        );
        animator.update(8.0);
        assert!(
            !animator.holds_finished_one_shot(),
            "the blend is done and only the loop is left"
        );
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

        assert!(animator.transition.is_none());
    }

    /// Retail's merge stores the weighted sum raw, so half-way between two layers whose
    /// bones sit 90 degrees apart the quaternion has magnitude cos(22.5°) — a normalising
    /// merge would rescale that back to unit and quietly undo the squash retail keeps.
    #[test]
    fn a_layer_merge_keeps_the_magnitude_retail_stores() {
        let (a, b) = ([0.0, 0.0, 0.0, 1.0], yaw_quat(90.0));
        let merged = merge_layer_rotation(a, b, 0.5);
        let raw = [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
            (a[3] + b[3]) * 0.5,
        ];
        for i in 0..4 {
            assert!(
                (merged[i] - raw[i]).abs() < 1e-7,
                "component {i}: merged {:?} is not the stored sum {raw:?}",
                merged
            );
        }
        let mag = merged[0] * merged[0]
            + merged[1] * merged[1]
            + merged[2] * merged[2]
            + merged[3] * merged[3];
        let expected = 22.5f32.to_radians().cos();
        assert!(
            (mag.sqrt() - expected).abs() < 1e-6,
            "magnitude {} is not retail's cos(22.5°) = {expected}",
            mag.sqrt()
        );
    }

    /// `t == 1` is a copy, not a blend: even when the incoming quaternion sits on the far
    /// hemisphere it lands unflipped.
    #[test]
    fn a_complete_merge_copies_the_incoming_quaternion_unflipped() {
        let b = yaw_quat(40.0).map(|c| -c);
        let a = [0.0, 0.0, 0.0, 1.0];
        assert!(a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3] < 0.0);
        assert_eq!(merge_layer_rotation(a, b, 1.0), b);
    }

    fn yaw_quat(deg: f32) -> [f32; 4] {
        let h = deg.to_radians() / 2.0;
        [0.0, h.sin(), 0.0, h.cos()]
    }

    /// Retail's merge has no arc choice at all: a joint turning +100 degrees to -100
    /// crosses the back whether or not that looks wrong to us, because `FFXiMain.dll
    /// retail-2026-09` RVA 0x33220 only ever flips the sign on a negative dot product.
    #[test]
    fn every_blend_takes_the_short_arc_whatever_the_twist() {
        let merged = merge_layer_rotation(yaw_quat(100.0), yaw_quat(-100.0), 0.5);
        assert!(
            merged[3].abs() < 0.2,
            "the mid-blend must sit on the far side, like retail: {merged:?}"
        );
    }
}
