use std::sync::Arc;

use ffxi_dat::event_dat::{EventBlock, EventDat, ZONE_PLAYER_ACTOR};

use crate::cue::EventCue;

use super::{ActorLookup, EventVm, PendingTag, StepResult};

// research/XiEvents/OpCodes/0x001F.md CodeMOVE; 0x005A.md CodeMOVE2;
// 0x0032.md MainSpeed; 0x0047.md FUNC_XiEvent_OpCode_0x0047.
pub const EVENT_COORD_UNITS: f32 = 1000.0;
pub const EVENT_HEADING_UNITS: f32 = 4096.0;
pub const EVENT_SPEED_SCALE: f32 = 0.1;
/// Retail's ReqStack holds 16 requests per actor (research/XiEvents/Event VM
/// Structures.md xievent_t::ReqStack); a push onto a full stack is the "no free
/// slot" case that makes REQSET yield.
const REQUEST_STACK_LIMIT: usize = 16;
const OP_SPEED: u8 = 0x32;
const OP_GET_POSITION: u8 = 0x3B;
const OP_REQSET: u8 = 0x27;
const OP_REQSET_CHECKED: u8 = 0x28;
const OP_REQUEST_WAIT: u8 = 0x29;
const OP_REQWAIT: u8 = 0x2A;
const OP_MOVE: u8 = 0x1F;
const OP_CODE_MOVE2: u8 = 0x5A;
const OP_POSITION_UPDATE: u8 = 0x47;
const OP_SET_EVENT_POS: u8 = 0x37;
const OP_SET_FACING: u8 = 0x39;
const OP_DTURA: u8 = 0x4A;
const OP_STOP_ACTION: u8 = 0x5E;
const OP_STOP_NAMED_ACTION: u8 = 0x6B;
const OP_LOOKAT: u8 = 0x79;
const OP_LOOK_AND_TALK: u8 = 0x1E;
const ENTITY_POSITION_BASE: u32 = 0x7F00;
const PLAYER_POSITION_BASE: u32 = 0x7F80;

// Operand offsets from the opcode byte, per research/XiEvents/OpCodes/*.md.
const REQSET_PRIORITY_OFS: usize = 1; // 0x0027 / 0x0028 / 0x0029
const REQSET_ACTOR_OFS: usize = 2;
const REQSET_TAG_OFS: usize = 6;
const REQWAIT_PRIORITY_OFS: usize = 1; // 0x002A
const MOVE_GOAL_OFS: usize = 2; // 0x001F case 0 (x @2, z @4, y @6)
const SET_EVENT_POS_X_OFS: usize = 1; // 0x0037 (z @3, y @5, heading @7)
const SET_FACING_OFS: usize = 1; // 0x0039
const DTURA_ACTOR_OFS: usize = 1; // 0x004A (target @5)
const DTURA_TARGET_OFS: usize = 5;
const STOP_ACTION_KEY_OFS: usize = 1; // 0x005E / 0x006B
const STOP_NAMED_ACTOR_OFS: usize = 5; // 0x006B
const LOOKAT_CASE_OFS: usize = 1; // 0x0079
const LOOKAT_ACTOR_OFS: usize = 2;
const LOOKAT_TARGET_OFS: usize = 6;
const LOOK_AND_TALK_TARGET_OFS: usize = 1; // 0x001E

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub heading: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneAction {
    PlayerPosition(EventPosition),
    PositionUpdate {
        position: EventPosition,
        end_para: u32,
    },
}

/// The priority retail's XiEventInit gives the initial run of an entity's own
/// block (research/XiEvents/Event VM Functions.md); the owner-block children
/// spawned at event start use it.
const OWNER_REQUEST_PRIORITY: u8 = 16;

/// One queued request on an actor's ReqStack: the child VM that runs it and
/// the priority its requester gave it. Lower is more important; a fresh lower
/// number preempts, and the preempted child resumes where it stopped because
/// it keeps its own exec pointer (research/XiEvents/Event VM Functions.md
/// XiEvent::ReqSet; Event VM Structures.md RunPos).
struct ActorRequest {
    priority: u8,
    tag: u8,
    vm: Box<EventVm>,
}

/// An actor's ReqStack. Retail keeps 16 slots per actor and runs the
/// lowest-numbered pending request each tick (research/XiEvents/Event VM
/// Structures.md RunPos).
struct ActorStack {
    actor: u32,
    requests: Vec<ActorRequest>,
}

pub(super) struct Scene {
    dat: Arc<EventDat>,
    pub(super) actor: u32,
    player: EventPosition,
    /// Raw 0x32 MainSpeed work-slot operand; yalms/sec is
    /// `speed as f32 * EVENT_SPEED_SCALE`.
    speed: i32,
    motion: Option<EventPosition>,
    pending_position: bool,
    pending_event: bool,
    controls_position: bool,
    held: bool,
    stacks: Vec<ActorStack>,
    /// The (actor, request index) of the child holding the open dialog frame:
    /// retail's CliEventMessOpenFlag is one global flag shared by every
    /// entity's VM (research/XiEvents/OpCodes/0x001D.md, 0x0023.md), so a
    /// child's PRINT_MSG parks the whole event and the player's response must
    /// reach that child.
    dialog_child: Option<(u32, usize)>,
}

impl EventVm {
    pub fn attach_scene(&mut self, dat: Arc<EventDat>, actor: u32, player: EventPosition) {
        self.scene = Some(Scene {
            dat,
            actor,
            player,
            speed: 0,
            motion: None,
            pending_position: false,
            pending_event: false,
            controls_position: false,
            held: false,
            stacks: Vec::new(),
            dialog_child: None,
        });
    }

    pub fn controls_player_position(&self) -> bool {
        self.scene.as_ref().is_some_and(|s| s.controls_position)
    }

    pub fn controlled_position(&self) -> Option<EventPosition> {
        self.scene
            .as_ref()
            .filter(|_| self.controls_player_position())
            .map(|scene| scene.player)
    }

    pub fn take_scene_actions(&mut self) -> Vec<SceneAction> {
        std::mem::take(&mut self.scene_actions)
    }

    pub fn acknowledge_position(&mut self, position: EventPosition) {
        if let Some(scene) = &mut self.scene {
            scene.player = position;
            if scene.pending_position {
                self.scene_actions
                    .push(SceneAction::PlayerPosition(position));
            }
            scene.pending_position = false;
            // A child parked on its own 0x47 round-trip releases against the
            // same server ack (research/XiEvents/OpCodes/0x0047.md).
            for stack in &mut scene.stacks {
                for request in &mut stack.requests {
                    request.vm.acknowledge_position(position);
                }
            }
        }
    }

    pub fn reject_position(&mut self) {
        if let Some(scene) = &self.scene {
            self.acknowledge_position(scene.player);
        }
    }

    pub fn acknowledge_event(&mut self) {
        if let Some(scene) = &mut self.scene {
            scene.pending_event = false;
            for stack in &mut scene.stacks {
                for request in &mut stack.requests {
                    request.vm.acknowledge_event();
                }
            }
        }
    }

    /// True while the host must keep driving this VM: parked on a scene hold,
    /// or any actor request stack still holds work. A fresh un-stepped child
    /// reports no wait of its own, so "any request" (not "any waiting request")
    /// is what keeps the first frame from deadlocking.
    pub(super) fn scene_waiting(&self) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|s| s.held || !s.stacks.is_empty())
    }

    pub(super) fn resume_scene(&mut self) {
        if let Some(scene) = &mut self.scene {
            scene.held = false;
        }
    }

    /// Run one frame of every actor request stack: each stack steps its
    /// lowest-priority-number request once, cues and scene actions bubble up
    /// with the child's actor as the event entity for cue resolution. The
    /// Work_Zone is one shared cell across every VM in the event, so a
    /// child's write is visible to the master and to siblings without a copy
    /// (work_local stays per child). A child that parks on a dialog frame
    /// surfaces it to the host via [`Scene::dialog_child`] instead of being
    /// dropped, because retail's single global CliEventMessOpenFlag
    /// (research/XiEvents/OpCodes/0x001D.md) keeps the whole event parked
    /// until the player answers that child; requests that stop on an
    /// unrunnable opcode are still dropped.
    pub(super) fn step_stacks(&mut self) -> Option<StepResult> {
        // Collect the active (actor, request index) pairs under one immutable
        // borrow; each step below needs &mut self.
        let Some(scene) = &self.scene else {
            return None;
        };
        let active = scene
            .stacks
            .iter()
            .filter_map(|stack| {
                stack
                    .requests
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, r)| r.priority)
                    .map(|(i, _)| (stack.actor, i))
            })
            .collect::<Vec<_>>();
        let mut surfaced: Option<StepResult> = None;
        for (actor, index) in active {
            let result = self.child_step(actor, index);
            match result {
                StepResult::Done => self.remove_request(actor, index),
                StepResult::AwaitMessage(_) | StepResult::AwaitChoice(_) => {
                    // A frame is already open: the master's own, or one another
                    // child surfaced this pass. Retail's single global flag holds
                    // one frame at a time and authored events sequence around it
                    // with REQWAIT/MESWAIT, so a second parker is dropped rather
                    // than stalling the stack behind it.
                    if self.pending_message.is_some()
                        || self.pending_choice.is_some()
                        || surfaced.is_some()
                    {
                        tracing::warn!(
                            target: "ffxi_event::vm",
                            actor,
                            ?result,
                            "dropping a child request that parked on a dialog \
                             while another frame is open"
                        );
                        self.remove_request(actor, index);
                    } else {
                        self.scene.as_mut().unwrap().dialog_child = Some((actor, index));
                        surfaced = Some(result);
                    }
                }
                StepResult::AwaitMessageAck | StepResult::AwaitServerAck(_) => {
                    tracing::debug!(
                        target: "ffxi_event::vm",
                        actor,
                        ?result,
                        "a child request parked on a server round-trip this \
                         scene does not author"
                    );
                }
                StepResult::Unimplemented(op) | StepResult::Spun(op) => {
                    tracing::debug!(
                        target: "ffxi_event::vm",
                        actor,
                        op = format!("0x{op:02X}"),
                        "dropping a child request that stopped on an opcode \
                         this VM does not run"
                    );
                    // The drop must be real: a zombie left on the stack keeps
                    // REQWAIT/REQEW parked on it forever.
                    self.remove_request(actor, index);
                }
                StepResult::Waiting | StepResult::Cancelled => {}
            }
        }
        surfaced
    }

    /// Tick every actor request stack with the host clock.
    pub(super) fn tick_stacks(&mut self, dt: f32) {
        let Some(scene) = &mut self.scene else { return };
        for stack in &mut scene.stacks {
            for request in &mut stack.requests {
                request.vm.tick(dt);
            }
        }
    }

    /// Run the host clock into the player's MOVE lerp while a scene hold is
    /// parked on it; each step publishes the new position as a scene action.
    pub(super) fn tick_scene(&mut self, dt: f32) {
        let Some(scene) = &mut self.scene else { return };
        if !scene.held {
            return;
        }
        let Some(goal) = scene.motion else { return };
        let dx = (goal.x - scene.player.x) as f32;
        let dz = (goal.z - scene.player.z) as f32;
        let distance = dx.hypot(dz);
        let travel = scene.speed as f32 * EVENT_SPEED_SCALE * dt * EVENT_COORD_UNITS;
        scene.player.heading =
            ((-dz).atan2(dx) / std::f32::consts::TAU * EVENT_HEADING_UNITS) as i32;
        if distance <= travel {
            scene.player.x = goal.x;
            scene.player.z = goal.z;
            scene.motion = None;
        } else if distance > 0.0 {
            scene.player.x += (dx / distance * travel) as i32;
            scene.player.z += (dz / distance * travel) as i32;
        }
        scene.player.y = goal.y;
        self.scene_actions
            .push(SceneAction::PlayerPosition(scene.player));
    }

    /// Step one child once and bubble its cues, scene actions and zone writes.
    fn child_step(&mut self, actor: u32, index: usize) -> StepResult {
        // The child is reached through the `scene` field alone so the cue and
        // zone bubbling below can borrow the sibling fields while it lives.
        let vm = &mut self
            .scene
            .as_mut()
            .unwrap()
            .stacks
            .iter_mut()
            .find(|s| s.actor == actor)
            .unwrap()
            .requests[index]
            .vm;
        // Host-armed holds live on the VM that published the cue (the master);
        // a child's WAIT* and MOVE case 1 park against them too.
        vm.action_holds = self.action_holds.clone();
        vm.move_holds = self.move_holds.clone();
        let result = vm.step();
        for cue in vm.take_cues() {
            self.cues.push(cue.resolve_event_actor(ActorLookup(actor)));
        }
        self.scene_actions.extend(vm.take_scene_actions());
        self.param_len = vm.param_len;
        // The local player's child owns the position this scene reports: mirror
        // its tracked position and control flag up so a finished event still
        // carries it (research/XiEvents/OpCodes/0x0047.md). NPC children keep
        // their own.
        let mirror = if actor == ZONE_PLAYER_ACTOR {
            vm.scene.as_ref().map(|s| (s.player, s.controls_position))
        } else {
            None
        };
        if let Some((player, controls)) = mirror {
            let scene = self.scene.as_mut().unwrap();
            scene.player = player;
            scene.controls_position |= controls;
        }
        result
    }

    pub(super) fn child_mut(&mut self, actor: u32, index: usize) -> &mut EventVm {
        &mut self
            .scene
            .as_mut()
            .unwrap()
            .stacks
            .iter_mut()
            .find(|s| s.actor == actor)
            .unwrap()
            .requests[index]
            .vm
    }

    /// Run `f` over the request VMs on this scene's actor stacks (this VM's
    /// direct children). Recursion into a child's own children is the
    /// closure's job: the fan-out closures in [`EventVm::ack_server`],
    /// [`EventVm::apply_pending_num`] and [`EventVm::apply_pending_str`]
    /// recurse by calling the same fan-out on each child.
    pub(super) fn for_each_child_vm(&mut self, f: &mut impl FnMut(&mut EventVm)) {
        let Some(scene) = &mut self.scene else { return };
        for stack in &mut scene.stacks {
            for request in &mut stack.requests {
                f(&mut request.vm);
            }
        }
    }

    /// The pending tag held on any descendant request VM, if any: each
    /// request's own [`EventVm::pending_tag`] looks into its children, so
    /// this reaches the whole tree. The tag is one global per event in
    /// retail, so the holder can be a child.
    pub(super) fn child_pending_tag(&self) -> Option<&PendingTag> {
        let scene = self.scene.as_ref()?;
        scene.stacks.iter().find_map(|stack| {
            stack
                .requests
                .iter()
                .find_map(|request| request.vm.pending_tag())
        })
    }

    /// The (actor, request index) of the child holding the open dialog frame,
    /// if one is parked on it; see [`Scene::dialog_child`].
    pub(super) fn open_frame_holder(&self) -> Option<(u32, usize)> {
        self.scene.as_ref().and_then(|s| s.dialog_child)
    }

    /// The target actor a REQSET-family opcode names: the local player maps to
    /// the zone block, the event entity selector to this scene's own actor, and
    /// everything else is its literal server id.
    fn reqset_actor(&self, target: ActorLookup) -> u32 {
        let scene = self.scene.as_ref().unwrap();
        if target.is_local_player() {
            ZONE_PLAYER_ACTOR
        } else if target.is_event_entity() {
            scene.actor
        } else {
            target.server_id().unwrap_or(scene.actor)
        }
    }

    /// True when `tag` is already queued or running on the actor's stack.
    /// Tag 0 is retail's "no tag" sentinel: `XiEvent::ReqSet` walks all 16
    /// ReqStack slots and returns 0 on any matching TagNum, including the
    /// zeroed TagNum of unused and completed slots (research/XiEvents/Event VM
    /// Functions.md ReqSet), so REQSET(tag=0) is a no-op at any actor whose
    /// stack is not full; only a full stack defers to the yield path.
    fn request_queued(&self, actor: u32, tag: u8) -> bool {
        if tag == 0 && !self.request_stack_full(actor) {
            return true;
        }
        self.scene.as_ref().is_some_and(|s| {
            s.stacks
                .iter()
                .any(|st| st.actor == actor && st.requests.iter().any(|r| r.tag == tag))
        })
    }

    /// True when the actor's stack holds `REQUEST_STACK_LIMIT` requests.
    fn request_stack_full(&self, actor: u32) -> bool {
        self.scene.as_ref().is_some_and(|s| {
            s.stacks
                .iter()
                .find(|st| st.actor == actor)
                .is_some_and(|st| st.requests.len() >= REQUEST_STACK_LIMIT)
        })
    }

    /// True while the target's stack holds any request at priority <= `priority`
    /// (research/XiEvents/Event VM Functions.md XiEvent::GetReqLevel returns 1
    /// only when every slot is numerically above it).
    fn request_at_or_below(&self, actor: u32, priority: u8) -> bool {
        self.scene.as_ref().is_some_and(|s| {
            s.stacks
                .iter()
                .find(|st| st.actor == actor)
                .is_some_and(|st| st.requests.iter().any(|r| r.priority <= priority))
        })
    }

    /// Push `tag` onto the actor's stack, starting a child at that tag index of
    /// the actor's own block. Returns false when nothing was pushed: the tag is
    /// already queued there (ReqSet returns 0), or the actor has no block or no
    /// entry at that index.
    fn push_request(&mut self, actor: u32, priority: u8, tag: u8) -> bool {
        if self.request_queued(actor, tag) {
            return false;
        }
        let scene = match &self.scene {
            Some(s) => s,
            None => return false,
        };
        let block = match scene.dat.block_for_actor(actor) {
            Some(b) => b,
            None => {
                tracing::debug!(target: "ffxi_event::vm", actor, tag, "REQSET target has no event block");
                return false;
            }
        };
        // research/XiEvents/Event VM Functions.md XiEvent::ReqSet indexes
        // TagOffset by the tag byte, including entries whose event id is a
        // placeholder.
        let entry = match block.event_offsets.get(tag as usize).copied() {
            Some(e) => e,
            None => {
                tracing::debug!(target: "ffxi_event::vm", actor, tag, "REQSET tag index out of range");
                return false;
            }
        };
        let dat = scene.dat.clone();
        // A child for the local player starts from the master's tracked
        // position; NPC children start at zero and their ActorMove cues carry
        // the goal only, so the renderer walks from where the entity stands.
        let player = if actor == ZONE_PLAYER_ACTOR {
            scene.player
        } else {
            EventPosition::default()
        };
        let mut child = EventVm::start_at_shared(
            block,
            entry as usize,
            self.speaker_index,
            self.params(),
            Arc::clone(&self.work_zone),
        );
        child.actor_types = self.actor_types.clone();
        child.attach_scene(dat, actor, player);
        let stacks = &mut self.scene.as_mut().unwrap().stacks;
        match stacks.iter_mut().find(|s| s.actor == actor) {
            Some(stack) => stack.requests.push(ActorRequest {
                priority,
                tag,
                vm: Box::new(child),
            }),
            None => stacks.push(ActorStack {
                actor,
                requests: vec![ActorRequest {
                    priority,
                    tag,
                    vm: Box::new(child),
                }],
            }),
        }
        true
    }

    /// Spawn a child VM for `block`'s program at `entry` (the owner block's
    /// own exact event entry) onto the scene's request stacks, so a
    /// multi-owner event runs every owner's program in parallel from event
    /// start. Retail's InitEvent2 prepares each valid entity and XiEventInit
    /// starts its own block on its own ReqStack (research/XiEvents/Event VM
    /// Functions.md); this is that, at event start, where [`push_request`]
    /// does it mid-program from a REQSET. The child's cues bubble up with the
    /// block's actor as the event entity; the master stays alive (its
    /// [`EventVm::finish_result`] waits on the scene's stacks) until every
    /// owner drains. No-op without an attached scene. The child's `tag` is 0,
    /// retail's reserved "no tag": the ReqStack slots start zeroed and
    /// XiEventInit sets only the event index and priority 16, the 0x0000
    /// reset writes `TagNum = 0` back on completion, and `XiEvent::ReqSet`
    /// dedupes on `TagNum` across all 16 slots — so a REQSET of tag 0 at this
    /// actor is skipped in retail too, while (and after) the owner child runs.
    /// That skip is the tag-0 branch of [`Self::request_queued`] (an actor
    /// whose stack is not full always has a zeroed slot to match), not a
    /// property of this slot (research/XiEvents/Event VM Functions.md
    /// XiEventInit, XiEvent::ReqSet; research/XiEvents/OpCodes/0x0000.md).
    pub fn spawn_owner(&mut self, block: &EventBlock, entry: usize) {
        let Some(scene) = &self.scene else { return };
        let actor = block.actor;
        let dat = scene.dat.clone();
        let player = if actor == ZONE_PLAYER_ACTOR {
            scene.player
        } else {
            EventPosition::default()
        };
        let mut child = EventVm::start_at_shared(
            block,
            entry,
            self.speaker_index,
            self.params(),
            Arc::clone(&self.work_zone),
        );
        child.actor_types = self.actor_types.clone();
        child.attach_scene(dat, actor, player);
        let stacks = &mut self.scene.as_mut().unwrap().stacks;
        match stacks.iter_mut().find(|s| s.actor == actor) {
            Some(stack) => stack.requests.push(ActorRequest {
                priority: OWNER_REQUEST_PRIORITY,
                tag: 0,
                vm: Box::new(child),
            }),
            None => stacks.push(ActorStack {
                actor,
                requests: vec![ActorRequest {
                    priority: OWNER_REQUEST_PRIORITY,
                    tag: 0,
                    vm: Box::new(child),
                }],
            }),
        }
    }

    /// The entity/player position accessor a `getworkofs` value selects, when a
    /// scene is attached: 0x7F00..0x7F03 on the zone block and 0x7F80..0x7F83
    /// anywhere read x, y, z, heading of the tracked player position.
    pub(super) fn scene_operand(&self, operand: u32) -> Option<i32> {
        let scene = self.scene.as_ref()?;
        let index = if scene.actor == ZONE_PLAYER_ACTOR
            && (ENTITY_POSITION_BASE..ENTITY_POSITION_BASE + 4).contains(&operand)
        {
            operand - ENTITY_POSITION_BASE
        } else if (PLAYER_POSITION_BASE..PLAYER_POSITION_BASE + 4).contains(&operand) {
            operand - PLAYER_POSITION_BASE
        } else {
            return None;
        };
        Some(
            [
                scene.player.x,
                scene.player.y,
                scene.player.z,
                scene.player.heading,
            ][index as usize],
        )
    }

    /// Remove a finished request from its stack; an actor whose last request
    /// leaves is pruned so `scene_waiting` sees no remaining work. A child that
    /// held the open dialog frame releases it with its slot.
    fn remove_request(&mut self, actor: u32, index: usize) {
        let scene = self.scene.as_mut().unwrap();
        if scene.dialog_child == Some((actor, index)) {
            scene.dialog_child = None;
        }
        if let Some(stack) = scene.stacks.iter_mut().find(|s| s.actor == actor) {
            stack.requests.remove(index);
        }
        scene.stacks.retain(|s| !s.requests.is_empty());
    }

    pub(super) fn scene_opcode(&mut self, op: u8) -> Option<StepResult> {
        match op {
            // XiEvent ReqSet family (research/XiEvents/OpCodes/0x0027.md,
            // 0x0028.md): push the tag onto the target actor's stack and run on.
            // Only a full stack yields (ReqSet returns "no free slot"); an
            // already-queued tag is a no-op that advances.
            OP_REQSET | OP_REQSET_CHECKED => {
                let priority = self.byte_at(REQSET_PRIORITY_OFS);
                let actor = self.reqset_actor(ActorLookup(self.eventgetcode2(REQSET_ACTOR_OFS)));
                let tag = self.byte_at(REQSET_TAG_OFS);
                if !self.request_queued(actor, tag) && self.request_stack_full(actor) {
                    tracing::debug!(target: "ffxi_event::vm", actor, tag, "REQSET stack is full; yielding");
                    self.scene.as_mut().unwrap().held = true;
                    return Some(StepResult::Waiting);
                }
                let _ = self.push_request(actor, priority, tag);
                self.advance(op);
            }
            // XiEvent REQEW (research/XiEvents/OpCodes/0x0029.md): push the tag
            // if it is not queued yet, then hold while that tag still sits on
            // the target's stack (GetReqStatus != -1). Retail keeps the wait in
            // ReqStack[RunPos].ReqFlag across ticks; this re-evaluates fresh each
            // step because resume_scene clears the hold before re-running this,
            // so `req_wait` records the request already issued instead of
            // queueing a second child when the first completes.
            OP_REQUEST_WAIT => {
                let priority = self.byte_at(REQSET_PRIORITY_OFS);
                let actor = self.reqset_actor(ActorLookup(self.eventgetcode2(REQSET_ACTOR_OFS)));
                let tag = self.byte_at(REQSET_TAG_OFS);
                if self.req_wait == Some((actor, tag)) {
                    // The parked re-run of this opcode: hold until the original
                    // request leaves the target's stack, then advance.
                    if !self.request_queued(actor, tag) {
                        self.req_wait = None;
                    }
                } else if !self.request_queued(actor, tag)
                    && self.push_request(actor, priority, tag)
                {
                    // Retail's ReqSet returns 0 for an already-queued tag and the
                    // opcode advances without waiting (research/XiEvents/OpCodes/
                    // 0x0029.md).
                    self.req_wait = Some((actor, tag));
                }
                if self.req_wait == Some((actor, tag)) {
                    self.scene.as_mut().unwrap().held = true;
                    return Some(StepResult::Waiting);
                }
                self.advance(op);
            }
            // XiEvent REQWAIT (research/XiEvents/OpCodes/0x002A.md): hold while
            // the target's stack holds any request at priority <= the given
            // byte.
            OP_REQWAIT => {
                let priority = self.byte_at(REQWAIT_PRIORITY_OFS);
                let actor = self.reqset_actor(ActorLookup(self.eventgetcode2(REQSET_ACTOR_OFS)));
                if self.request_at_or_below(actor, priority) {
                    self.scene.as_mut().unwrap().held = true;
                    return Some(StepResult::Waiting);
                }
                self.advance(op);
            }
            OP_SPEED => {
                let speed = self.getworkofs(1, 0);
                self.scene.as_mut().unwrap().speed = speed;
                self.advance(op);
            }
            OP_GET_POSITION => {
                let target = ActorLookup(self.eventgetcode2(1));
                if target.is_local_player() {
                    let p = self.scene.as_ref().unwrap().player;
                    self.setworkofs(5, p.x, 0);
                    self.setworkofs(7, p.z, 0);
                    self.setworkofs(9, p.y, 0);
                }
                self.advance(op);
            }
            OP_MOVE | OP_CODE_MOVE2 if self.scene.as_ref().unwrap().actor == ZONE_PLAYER_ACTOR => {
                if self.byte_at(1) == 0 {
                    let goal = self.position_operands(MOVE_GOAL_OFS, false);
                    self.scene.as_mut().unwrap().motion = Some(goal);
                    self.scene.as_mut().unwrap().controls_position = true;
                    self.advance(op);
                } else if self.byte_at(1) == 1 {
                    if self.scene.as_ref().unwrap().motion.is_some() {
                        self.scene.as_mut().unwrap().held = true;
                        return Some(StepResult::Waiting);
                    }
                    self.scene.as_mut().unwrap().held = false;
                    self.advance(op);
                } else {
                    return Some(StepResult::Unimplemented(op));
                }
            }
            // 0x1F MOVE / 0x5A CodeMOVE2 on a non-player actor: case 0 walks
            // the event entity to its goal (the host arms the arrival hold from
            // distance/speed), case 1 holds while that move still has frames
            // left. 0x5A is retail's uncalibrated twin of 0x1F
            // (research/XiEvents/OpCodes/0x005A.md).
            OP_MOVE | OP_CODE_MOVE2 => {
                if self.byte_at(1) == 0 {
                    let goal = self.position_operands(MOVE_GOAL_OFS, false);
                    let speed = self.scene.as_ref().unwrap().speed;
                    self.cues.push(EventCue::ActorMove {
                        actor: ActorLookup::EVENT_ENTITY,
                        goal,
                        speed,
                    });
                    self.advance(op);
                } else if self.byte_at(1) == 1 {
                    let actor = ActorLookup(self.scene.as_ref().unwrap().actor);
                    if self.move_running(actor) {
                        self.parked_on_move_hold = true;
                        return Some(StepResult::Waiting);
                    }
                    self.parked_on_move_hold = false;
                    self.advance(op);
                } else {
                    return Some(StepResult::Unimplemented(op));
                }
            }
            // 0x37 on a non-player actor: set the event entity's position. The
            // player-actor version keeps its width skip (the server round trip
            // owns that path).
            OP_SET_EVENT_POS if self.scene.as_ref().unwrap().actor != ZONE_PLAYER_ACTOR => {
                let position = self.position_operands(SET_EVENT_POS_X_OFS, true);
                self.cues.push(EventCue::ActorPlace {
                    actor: ActorLookup::EVENT_ENTITY,
                    position,
                });
                self.advance(op);
            }
            // 0x39 on a non-player actor: set the event entity's facing.
            OP_SET_FACING if self.scene.as_ref().unwrap().actor != ZONE_PLAYER_ACTOR => {
                let heading = self.getworkofs(SET_FACING_OFS, 0);
                self.cues.push(EventCue::ActorFace {
                    actor: ActorLookup::EVENT_ENTITY,
                    heading,
                });
                self.advance(op);
            }
            // 0x4A DTURA: turn the first named actor toward the second.
            OP_DTURA => {
                let actor = ActorLookup(self.eventgetcode2(DTURA_ACTOR_OFS));
                let target = ActorLookup(self.eventgetcode2(DTURA_TARGET_OFS));
                self.cues.push(EventCue::ActorLookAt { actor, target });
                self.advance(op);
            }
            // 0x5E: stop the event entity's current action and return it to idle.
            OP_STOP_ACTION => {
                let key = (self.eventgetcode2(STOP_ACTION_KEY_OFS) != 0)
                    .then(|| self.fourcc_at(STOP_ACTION_KEY_OFS));
                self.cues.push(EventCue::ActorStopAction {
                    actor: ActorLookup::EVENT_ENTITY,
                    key,
                });
                self.advance(op);
            }
            // 0x6B: stop the named action on the second named actor.
            OP_STOP_NAMED_ACTION => {
                let actor = ActorLookup(self.eventgetcode2(STOP_NAMED_ACTOR_OFS));
                let key = (self.eventgetcode2(STOP_ACTION_KEY_OFS) != 0)
                    .then(|| self.fourcc_at(STOP_ACTION_KEY_OFS));
                self.cues.push(EventCue::ActorStopAction { actor, key });
                self.advance(op);
            }
            // 0x79 lookat case 0: turn the first named actor toward the second.
            OP_LOOKAT if self.byte_at(LOOKAT_CASE_OFS) == 0 => {
                let actor = ActorLookup(self.eventgetcode2(LOOKAT_ACTOR_OFS));
                let target = ActorLookup(self.eventgetcode2(LOOKAT_TARGET_OFS));
                self.cues.push(EventCue::ActorLookAt { actor, target });
                self.advance(op);
            }
            // 0x1E look-and-talk: the motion half turns the event entity toward
            // the named actor; the talk half is a separate message path.
            OP_LOOK_AND_TALK => {
                let target = ActorLookup(self.eventgetcode2(LOOK_AND_TALK_TARGET_OFS));
                self.cues.push(EventCue::ActorLookAt {
                    actor: ActorLookup::EVENT_ENTITY,
                    target,
                });
                self.advance(op);
            }
            OP_POSITION_UPDATE => {
                if self.byte_at(1) == 0 {
                    let position = self.position_operands(2, true);
                    self.scene_actions.push(SceneAction::PositionUpdate {
                        position,
                        end_para: self.work_zone(1) as u32,
                    });
                    self.scene.as_mut().unwrap().pending_position = true;
                    self.scene.as_mut().unwrap().controls_position = true;
                    self.scene.as_mut().unwrap().pending_event = true;
                    self.scene.as_mut().unwrap().held = true;
                    self.advance(op);
                    return Some(StepResult::Waiting);
                } else if self.byte_at(1) == 1 {
                    if self.scene.as_ref().unwrap().pending_position
                        || self.scene.as_ref().unwrap().pending_event
                    {
                        self.scene.as_mut().unwrap().held = true;
                        return Some(StepResult::Waiting);
                    }
                    self.scene.as_mut().unwrap().held = false;
                    self.advance(op);
                } else {
                    return Some(StepResult::Unimplemented(op));
                }
            }
            _ => self.advance(op),
        }
        None
    }

    fn position_operands(&self, start: usize, heading: bool) -> EventPosition {
        EventPosition {
            x: self.getworkofs(start, 0),
            z: self.getworkofs(start + 2, 0),
            y: self.getworkofs(start + 4, 0),
            heading: if heading {
                self.getworkofs(start + 6, 0)
            } else {
                0
            },
        }
    }

    pub(super) fn handles_scene_opcode(&self, op: u8) -> bool {
        self.scene.is_some()
            && matches!(
                op,
                OP_REQSET
                    | OP_REQSET_CHECKED
                    | OP_REQUEST_WAIT
                    | OP_REQWAIT
                    | OP_SPEED
                    | OP_GET_POSITION
                    | OP_MOVE
                    | OP_CODE_MOVE2
                    | OP_SET_EVENT_POS
                    | OP_SET_FACING
                    | OP_DTURA
                    | OP_STOP_ACTION
                    | OP_STOP_NAMED_ACTION
                    | OP_LOOKAT
                    | OP_LOOK_AND_TALK
                    | OP_POSITION_UPDATE
            )
    }
}
