//! `ffxi-actor` — pure-Rust port of XIM's runtime entity pose, animation, and
//! animation-selection logic. No Bevy: every module is headlessly unit-testable.
//!
//! - [`skeleton_instance`] ports `xim/resource/SkeletonInstance.kt` (per-joint
//!   world pose matrices the skinned shader multiplies — no inverse bind).
//! - [`animation`] ports `xim/poc/SkeletonAnimator.kt` (per-slot context,
//!   transitions, and the 8-slot coordinator with cross-slot blending).
//! - [`actor_state`] ports the animation-*selection* methods of
//!   `xim/poc/Actor.kt` (which parameterized `DatId` an actor should play).

pub mod actor_state;
pub mod animation;
pub mod skeleton_instance;

pub use glam::{Mat4, Quat, Vec3};
