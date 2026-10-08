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
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let (b0, b1, b2, b3) = if dot < 0.0 {
        (-b[0], -b[1], -b[2], -b[3])
    } else {
        (b[0], b[1], b[2], b[3])
    };
    let inv = 1.0 - t;
    [
        a[0] * inv + b0 * t,
        a[1] * inv + b1 * t,
        a[2] * inv + b2 * t,
        a[3] * inv + b3 * t,
    ]
}

pub fn interpolate_kf(a: &KeyFrameTransform, b: &KeyFrameTransform, t: f32) -> KeyFrameTransform {
    KeyFrameTransform {
        rotation: merge_layer_rotation(a.rotation, b.rotation, t),
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

/// One bone's crossfade sample when either side may not key it. A clip that does not key a bone writes
/// nothing for it, so a side with no key is **not** a bind-pose key: whichever live side keys the bone owns
/// it outright (`FFXiMain.dll retail-2026-09` RVA 0x1B230 samples a slot and writes only the bones its
/// motion keys; nothing blends a bone a layer never wrote). Blending against a stand-in instead walked every unkeyed bone
/// to bind over the fade, which is the floating weapon: a battle turn-in-place clip keys no weapon joint,
/// so each turn faded the weapon out of the hand.
fn interpolate_nullable(
    a: Option<&KeyFrameTransform>,
    b: Option<&KeyFrameTransform>,
    t: f32,
) -> Option<KeyFrameTransform> {
    match (a, b) {
        (None, None) => None,
        (Some(a), Some(b)) => Some(interpolate_kf(a, b, t)),
        // Only one side keys the bone: it owns the bone, at full weight. Blending against a stand-in
        // here is what walked unkeyed bones to bind mid-fade.
        (Some(a), None) | (None, Some(a)) => Some(*a),
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
}

impl Default for TransitionParams {
    fn default() -> Self {
        TransitionParams {
            transition_in_time: 7.5,
            transition_out_time: 7.5,
            eager_transition_out: false,
        }
    }
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
}

pub struct AnimationTransition {
    pub previous: PreviousSide,
    pub next: SkeletonAnimationContext,
    pub transition_duration: f32,
    progress: f32,
}

impl AnimationTransition {
    pub fn new(
        previous: PreviousSide,
        next: SkeletonAnimationContext,
        transition_duration: f32,
    ) -> Self {
        AnimationTransition {
            previous,
            next,
            transition_duration,
            progress: 0.0,
        }
    }

    /// Every tick under the blend steps both layers and the in-flight request
    /// — gait clips keep running through a crossfade rather than pausing at
    /// their entry frame (`FFXiMain.dll retail-2026-09` sampler RVA 0x1B230).
    pub fn update(&mut self, elapsed_frames: f32) -> bool {
        self.previous.advance(elapsed_frames);
        self.next.advance(elapsed_frames);
        self.progress += elapsed_frames;
        self.is_complete()
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= self.transition_duration
    }

    /// What the merge sees for one bone this frame: the outgoing record, the incoming record and the weight.
    pub fn sides(
        &self,
        joint: usize,
    ) -> (Option<KeyFrameTransform>, Option<KeyFrameTransform>, f32) {
        (
            self.previous.get_joint_transform(joint),
            self.next.get_joint_transform(joint),
            self.progress / self.transition_duration,
        )
    }

    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        let t = self.progress / self.transition_duration;

        // Two sides only. Retail's merge (`FFXiMain.dll retail-2026-09` RVA 0x33220, dancer_engine.md
        // §4a-bis) is a single weighted sum of the outgoing and incoming bone records with the
        // shortest-arc sign flip; no third pose enters it. The idle-frame-0 waypoint this used to take
        // was xim's PoC idea (`research/xim` SkeletonAnimator.kt), has no counterparty anywhere in
        // FFXiMain, and measurably dragged held-weapon chains through the casual-idle bone records on
        // every Left/Right entry (gaps §G.12).
        let prev = self.previous.get_joint_transform(joint);
        let next = self.next.get_joint_transform(joint);
        interpolate_nullable(prev.as_ref(), next.as_ref(), t)
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

            let previous = match &self.transition {
                Some(t) => PreviousSide::Frozen(AnimationSnapshot::from_transition(t)),
                None => PreviousSide::Live(current.clone()),
            };

            self.transition = Some(AnimationTransition::new(
                previous,
                clone_context_at_frame0(&ctx),
                transition_duration,
            ));
        }

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

fn clone_context_at_frame0(ctx: &SkeletonAnimationContext) -> SkeletonAnimationContext {
    SkeletonAnimationContext::new(
        ctx.animation.clone(),
        ctx.loop_params,
        ctx.transition_params.clone(),
    )
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
