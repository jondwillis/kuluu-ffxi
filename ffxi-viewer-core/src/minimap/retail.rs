//! Retail-map minimap backend: load FFXI's stylized in-game map
//! texture (the `Ctrl+M` bitmap) for the current zone and publish it
//! on [`super::MinimapState::retail_image`].
//!
//! Scaffold only. The retail-map DAT format isn't parsed yet — that
//! work lives in `ffxi-dat::map_image` (task #6). Once a parser exists
//! this module loads + caches per zone-id.
//!
//! AGPL containment (per MEMORY.md `ffxi_agpl_containment.md` /
//! `xi_tinkerer_agpl_reference.md`): cross-reference xi-tinkerer's
//! reverse-engineering notes for algorithmic understanding only. Do
//! **not** link any xi-tinkerer crate from this code path.

use bevy::prelude::*;

/// Plugin registration. Empty today; the loader system lands once
/// `ffxi-dat::map_image` exists.
pub struct RetailBackendPlugin;

impl Plugin for RetailBackendPlugin {
    fn build(&self, _app: &mut App) {
        // intentionally empty — see module docs
    }
}
