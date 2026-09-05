//! Single-authority player walker (rebuild of the avian bridge + legacy sweep).
//!
//! One module owns horizontal slide, vertical position, falling, and dynamic
//! obstacles. Geometry queries stay on `MzbCollisionGeometry` (kuluu-render):
//! grid `cell_index`, column queries (`ground_raycast` / `ground_step`), and
//! the triangle contact helpers. No ECS inside [`step`] — pure over its
//! inputs so the test matrices can drive it headless.
//!
//! Layout: `consts` (one place, plan §2.7), `field` (the ramp field + support
//! probe), `sweep` (horizontal slide against walls), `obstacles` (doors +
//! mobs, rebuilt every fixed tick), `step` (the tick itself), `debug`
//! (FieldDebug resource + panel/gizmo plumbing).

pub mod consts;
pub mod debug;
pub mod field;
pub mod obstacles;
pub mod step;
pub mod sweep;

#[cfg(test)]
mod live_tests;

use bevy::prelude::*;

/// Per-entity push-through accrual: which mob the player is currently shoving,
/// and for how long (plan §2.5). Sustained pressure into the SAME mob past
/// PUSH_THROUGH_SECS excludes it until the pressure releases. The key is the
/// mob's wire entity id (`MobObstacle::id`).
#[derive(Default)]
pub struct PushThrough {
    target: Option<u32>,
    secs: f32,
}

impl PushThrough {
    /// Register a block against `mob` this tick; returns true once the mob has
    /// been pressed continuously for PUSH_THROUGH_SECS and should stop blocking.
    pub fn press(&mut self, mob: u32, dt: f32) -> bool {
        match self.target {
            Some(t) if t == mob => self.secs += dt,
            _ => {
                self.target = Some(mob);
                self.secs = dt;
            }
        }
        self.secs >= consts::PUSH_THROUGH_SECS
    }

    /// Pressure released (or a different obstacle took over): reset the clock.
    pub fn release(&mut self) {
        self.target = None;
        self.secs = 0.0;
    }

    /// This mob is currently excluded by sustained pressure.
    pub fn excluded(&self, mob: u32) -> bool {
        self.target == Some(mob) && self.secs >= consts::PUSH_THROUGH_SECS
    }

    /// The mob the clock is running against (None when released).
    pub fn target(&self) -> Option<u32> {
        self.target
    }
}

/// The walker's vertical mode (plan §2.3). Driven by input: `want_len == 0` is
/// Stopped this tick; Airborne persists until a landing, which picks the next
/// mode from that tick's input.
#[derive(Clone, Copy, Debug, Default)]
pub enum WalkMode {
    #[default]
    Stopped,
    Walking,
    /// Falling: `vy` in yalms/s (negative = down), integrated by FallModel.
    Airborne {
        vy: f32,
    },
}

/// Cross-tick walker state, held as a Local in dispatch (`DispatchLocals`).
#[derive(Default)]
pub struct Walker {
    pub mode: WalkMode,
    /// Push-through accrual against the mob currently being shoved.
    pub push_through: PushThrough,
    /// Fall feel (plan §0 Q3): fast and smooth, tuned by walking off ledges —
    /// swap for the real constant if the XiClient source ever turns up one.
    pub fall: consts::FallModel,
    /// Slew-limited envelope gradient carried across ticks (plan §2.2): a
    /// wobbly estimate or a fast 180 can't spike g.
    pub grad: bevy::math::Vec2,
}

/// One tick's outcome. `dx`/`dy` are the allowed horizontal displacement in
/// wire units; `feet_z` is where the walker puts its feet this tick (wire z,
/// grows down).
#[derive(Clone, Copy, Debug)]
pub struct StepResult {
    pub dx: f32,
    pub dy: f32,
    pub feet_z: f32,
    /// The mode after this tick's vertical pass.
    pub mode: WalkMode,
    /// What the vertical pass did (plan §3).
    pub decision: VerticalDecision,
}

/// What this tick's vertical pass did (plan §3). One per tick; the panel shows
/// the last two and step 4's live tests assert on it.
#[derive(Clone, Copy, Debug)]
pub enum VerticalDecision {
    /// Idle tick: settled toward h0 at speed (or held when already there).
    Stopped { settling: bool },
    /// Walking a staircase window: merged toward the envelope at speed.
    Ramp { g: bevy::math::Vec2, target: f32 },
    /// One riser in the window: climbed/descended it at speed as h0 moved.
    SingleStep { up: bool },
    /// Continuous surface (a slope): followed h0 at speed.
    Slope,
    /// Flat ground: no vertical move to make.
    Flat,
    /// Dead band: no ramp in the window, snapped to h0 instantly.
    Poof { delta: f32 },
    /// Gravity integrated; no landing this tick.
    Airborne { vy: f32 },
    /// Was airborne; a floor entered the swept band and we landed on it.
    Landed,
    /// A rise was rejected for the tick (body top would push into geometry).
    CeilingHold,
    /// The field saw a wall face ahead capping the chain: held h0, no rise —
    /// the sweep slides on it.
    WallAhead,
}

impl VerticalDecision {
    /// Compact ASCII label for the panel (the render crate can't name our types).
    pub fn label(&self) -> String {
        match self {
            Self::Stopped { settling } => {
                if *settling {
                    "Stopped(settle)".into()
                } else {
                    "Stopped(hold)".into()
                }
            }
            Self::Ramp { g, target } => format!("Ramp g=({:+.2},{:+.2}) t={:.2}", g.x, g.y, target),
            Self::SingleStep { up } => {
                if *up {
                    "StepUp".into()
                } else {
                    "StepDown".into()
                }
            }
            Self::Slope => "Slope".into(),
            Self::Flat => "Flat".into(),
            Self::Poof { delta } => format!("Poof d={:+.2}", delta),
            Self::Airborne { vy } => format!("Air vy={:+.2}", vy),
            Self::Landed => "Landed".into(),
            Self::CeilingHold => "CeilHold".into(),
            Self::WallAhead => "WallAhead".into(),
        }
    }
}

/// One fixed tick of the walker (wire coordinates at the boundary; see
/// `step.rs` for the pass order).
pub use step::step;

/// Registers the walker's resources and systems: the obstacle rebuild runs in
/// FixedUpdate before dispatch (the slot the avian collider syncs used); the
/// debug gizmo + snapshot systems run every frame.
pub struct WalkerPlugin;

impl Plugin for WalkerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<obstacles::ObstacleSet>();
        app.init_resource::<debug::FieldDebug>();
        app.init_resource::<debug::StairDebugZoneCache>();
        // Dynamic obstacles before the walker reads them (plan §2.5).
        app.add_systems(
            FixedUpdate,
            (
                obstacles::snapshot_mob_block_radius.before(obstacles::rebuild_obstacles_system),
                obstacles::rebuild_obstacles_system.before(super::input::dispatch_movement_system),
            )
                .run_if(in_state(super::AppPhase::InGame)),
        );
        // In-world ramp-field gizmos behind the `stair_draw` toggle.
        app.add_systems(
            Update,
            debug::draw_walker_field_gizmos.run_if(in_state(super::AppPhase::InGame)),
        );
        // Panel snapshot: FieldDebug -> StairDebugSnapshot every frame (the
        // render crate's stair_debug panel reads it).
        app.add_systems(
            Update,
            debug::update_stair_debug_snapshot_system.run_if(in_state(super::AppPhase::InGame)),
        );
    }
}
