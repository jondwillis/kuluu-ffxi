//! Top-down minimap backend: secondary orthographic `Camera3d` that
//! renders the loaded MZB geometry into a texture, **once per
//! zone-enter** (bake-once). Subsequent frames just read the cached
//! texture — no per-frame render cost.
//!
//! The "hide ceilings / roofs / tunnel-tops" trick is a positional
//! cull, not a depth shader: the camera sits at `aabb.max.y + margin`
//! looking straight down, with its `far` plane sized to exactly the
//! zone's height span. Anything *above* the camera is behind it (not
//! rendered); anything *below* `floor_y` is past `far` (also not
//! rendered). For multi-level zones the `top_cull_yalms` knob trims
//! additional layers off the top — see [`TopdownCullPolicy`].
//!
//! NOTE: this module currently registers its plugin but **does not yet
//! perform the bake**. The plumbing scaffold is here so the minimap UI
//! has a stable home; the actual render-to-texture system lands in a
//! follow-up commit.

use bevy::prelude::*;

/// How much of the zone's vertical extent (in yalms) to trim from the
/// top before rendering. Default `0.0` = render the full zone height
/// (single-pass, every ceiling visible from the camera's POV — which
/// for an overhead camera is exactly the ceilings the operator wants
/// hidden, so the default ≠ "what you want"; see follow-up).
///
/// A typical FFXI city like San d'Oria benefits from ~6 yalms of trim
/// (clips the upper floor of two-story buildings). Dungeon zones like
/// Pso'Xja need closer to 20 yalms because their tunnel ceilings sit
/// well above the floor.
///
/// Operator-tunable via `/minimap cull <N>` (slash command to be wired
/// in task #2).
#[derive(Resource, Debug, Clone, Copy)]
pub struct TopdownCullPolicy {
    pub top_cull_yalms: f32,
}

impl Default for TopdownCullPolicy {
    fn default() -> Self {
        Self { top_cull_yalms: 6.0 }
    }
}

/// Plugin registration. Just owns the cull policy resource today; the
/// bake camera + zone-enter watcher land in the next commit.
pub struct TopdownBackendPlugin;

impl Plugin for TopdownBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TopdownCullPolicy>();
    }
}
