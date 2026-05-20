//! Entity dot overlay. Reads [`crate::scene::TrackedEntities`] +
//! [`crate::components::IsSelf`] each frame, projects every entity into
//! minimap UV space via [`super::MinimapState::active_aabb`], and
//! spawns / updates child `Node`s under the [`super::MinimapOverlayLayer`]
//! container.
//!
//! Identical math for both backends — the AABB plumbed through
//! `MinimapState` is the only thing that differs.
//!
//! Scaffold only: today this system just no-ops if the AABB isn't set
//! yet. Dot spawning lands in task #1 after the top-down bake
//! populates an AABB.

use bevy::prelude::*;

use super::{MinimapMode, MinimapState};

/// Per-frame: reconcile dots for every tracked entity. Skips entirely
/// when no AABB is available (i.e. zone not yet baked / loaded).
pub fn update_minimap_overlay(
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
) {
    if state.active_aabb(*mode).is_none() {
        return;
    }
    // Real dot reconcile lands in task #1.
}
