//! FFXI client event/cutscene **bytecode VM**.
//!
//! FFXI cutscenes and NPC dialog are compiled bytecode shipped in per-zone event
//! DATs ([`ffxi_dat::event_dat`]); the server only sends a trigger (map packet
//! 0x32) and the client runs the local bytecode. This crate is the interpreter:
//! it reproduces the `XiEvent` VM (atom0s/XiEvents) as a steppable coroutine so
//! the async session can drive it, satisfying each yield (show a message, wait
//! for the player, …) against real dialog strings ([`ffxi_dat::dmsg`]).
//!
//! Opcode semantics are ported from `research/XiEvents/OpCodes/*.md` and the VM
//! function docs (a studied reference, not a build input). The implemented set
//! is the minimal dialog flow; unimplemented opcodes are skipped by their
//! documented size when safe, or stop the VM ([`StepResult::Unimplemented`])
//! when they would otherwise desync the exec pointer.

pub mod opcode_meta;
pub mod runner;
pub mod vm;

pub use runner::{DialogFrame, DialogRunner, DialogStep};
pub use vm::{EventChoice, EventMessage, EventVm, StepResult};
