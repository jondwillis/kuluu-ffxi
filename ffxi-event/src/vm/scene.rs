use std::sync::Arc;

use ffxi_dat::event_dat::{EventDat, ZONE_PLAYER_ACTOR};

use super::{ActorLookup, EventVm, StepResult};

// research/XiEvents/OpCodes/0x001F.md CodeMOVE; 0x0032.md MainSpeed;
// 0x0047.md FUNC_XiEvent_OpCode_0x0047.
pub const EVENT_COORD_UNITS: f32 = 1000.0;
pub const EVENT_HEADING_UNITS: f32 = 4096.0;
const EVENT_SPEED_SCALE: f32 = 0.1;
const REQUEST_STACK_LIMIT: usize = 16;
const OP_SPEED: u8 = 0x32;
const OP_GET_POSITION: u8 = 0x3B;
const OP_REQUEST_WAIT: u8 = 0x29;
const OP_MOVE: u8 = 0x1F;
const OP_POSITION_UPDATE: u8 = 0x47;
const ENTITY_POSITION_BASE: u32 = 0x7F00;
const PLAYER_POSITION_BASE: u32 = 0x7F80;

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

pub(super) struct Scene {
    dat: Arc<EventDat>,
    pub(super) actor: u32,
    player: EventPosition,
    speed: f32,
    motion: Option<EventPosition>,
    pending_position: bool,
    pending_event: bool,
    controls_position: bool,
    held: bool,
    depth: usize,
    child: Option<Box<EventVm>>,
}

impl EventVm {
    pub fn attach_scene(&mut self, dat: Arc<EventDat>, actor: u32, player: EventPosition) {
        self.scene = Some(Scene {
            dat,
            actor,
            player,
            speed: 0.0,
            motion: None,
            pending_position: false,
            pending_event: false,
            controls_position: false,
            held: false,
            depth: 0,
            child: None,
        });
    }

    pub(super) fn dismiss_child_message(&mut self) {
        if let Some(child) = self.scene.as_mut().and_then(|s| s.child.as_mut()) {
            child.dismiss_message();
        }
    }

    pub(super) fn select_child_choice(&mut self, index: Option<u32>) {
        if let Some(child) = self.scene.as_mut().and_then(|s| s.child.as_mut()) {
            child.select_choice(index);
        }
    }

    pub fn controls_player_position(&self) -> bool {
        self.scene.as_ref().is_some_and(|s| {
            s.controls_position
                || s.child
                    .as_ref()
                    .is_some_and(|c| c.controls_player_position())
        })
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
            if let Some(child) = &mut scene.child {
                child.acknowledge_position(position);
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
            if let Some(child) = &mut scene.child {
                child.acknowledge_event();
            }
        }
    }

    pub(super) fn scene_waiting(&self) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|s| s.held || s.child.as_ref().is_some_and(|c| c.is_waiting()))
    }

    pub(super) fn resume_scene(&mut self) {
        if let Some(scene) = &mut self.scene {
            scene.held = false;
        }
    }

    pub(super) fn step_child(&mut self) -> Option<StepResult> {
        let scene = self.scene.as_mut()?;
        let child = scene.child.as_mut()?;
        let result = child.step();
        self.cues.extend(child.take_cues());
        self.scene_actions.extend(child.take_scene_actions());
        self.work_zone = child.work_zone;
        self.param_len = child.param_len;
        if let Some(child_scene) = &child.scene {
            scene.player = child_scene.player;
            scene.controls_position |= child_scene.controls_position;
        }
        if result == StepResult::Done {
            scene.child = None;
            None
        } else {
            Some(result)
        }
    }

    pub(super) fn tick_scene(&mut self, dt: f32) {
        let Some(scene) = &mut self.scene else { return };
        if let Some(child) = &mut scene.child {
            child.tick(dt);
            return;
        }
        if !scene.held {
            return;
        }
        let Some(goal) = scene.motion else { return };
        let dx = (goal.x - scene.player.x) as f32;
        let dz = (goal.z - scene.player.z) as f32;
        let distance = dx.hypot(dz);
        let travel = scene.speed * dt * EVENT_COORD_UNITS;
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

    pub(super) fn scene_opcode(&mut self, op: u8) -> Option<StepResult> {
        match op {
            OP_REQUEST_WAIT => {
                let target = ActorLookup(self.eventgetcode2(2));
                let tag = self.byte_at(6) as usize;
                let scene = self.scene.as_ref().unwrap();
                let actor = if target.is_local_player() {
                    ZONE_PLAYER_ACTOR
                } else if target.is_event_entity() {
                    scene.actor
                } else {
                    target.server_id().unwrap_or(scene.actor)
                };
                // research/XiEvents/Event VM Functions.md XiEvent::ReqSet indexes
                // TagOffset by tag, including entries whose event id is a placeholder.
                let block = scene.dat.block_for_actor(actor);
                let entry = block.and_then(|b| b.event_offsets.get(tag).map(|&p| (b, p)));
                if let Some((block, entry)) = entry {
                    if scene.depth >= REQUEST_STACK_LIMIT {
                        return Some(StepResult::Spun(op));
                    }
                    let mut child =
                        EventVm::start_at(block, entry as usize, self.speaker_index, self.params());
                    child.work_zone = self.work_zone;
                    child.attach_scene(scene.dat.clone(), actor, scene.player);
                    child.scene.as_mut().unwrap().depth = scene.depth + 1;
                    self.scene.as_mut().unwrap().child = Some(Box::new(child));
                }
                self.advance(op);
            }
            OP_SPEED => {
                let speed = self.getworkofs(1, 0) as f32 * EVENT_SPEED_SCALE;
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
            OP_MOVE if self.scene.as_ref().unwrap().actor == ZONE_PLAYER_ACTOR => {
                if self.byte_at(1) == 0 {
                    let goal = self.position_operands(2, false);
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
                OP_REQUEST_WAIT | OP_SPEED | OP_GET_POSITION | OP_MOVE | OP_POSITION_UPDATE
            )
    }
}
