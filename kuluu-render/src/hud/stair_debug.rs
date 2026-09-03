use bevy::prelude::*;

use crate::hud::style::{self, theme};

#[derive(Component)]
pub struct StairDebugHud;

#[derive(Component)]
pub struct StairDebugHudText;

pub fn spawn_stair_debug_hud(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            StairDebugHud,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Px(10.0),
                width: Val::Px(520.0),
                max_height: Val::Percent(90.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::CURSOR),
            Visibility::Hidden,
            StairDebugPanelRoot,
        ))
        .with_children(|p| {
            p.spawn((
                StairDebugHudText,
                Text::new(""),
                style::text_font(11.0),
                TextColor(theme::TEXT),
            ));
        });
}

/// Reads FootprintDebug (owned by the input crate — kuluu-render can't
/// depend on kuluu, so we pull it via a shared resource type re-exported
/// by kuluu_render::stair_debug_view). We keep the render side ignorant of
/// FootprintDebug's field layout by pulling only the summary snapshot.
///
/// A separate system in the kuluu crate populates StairDebugSnapshot from
/// its FootprintDebug each frame.
pub fn update_stair_debug_hud(
    snap: Res<StairDebugSnapshot>,
    panels: Res<crate::hud::HudPanels>,
    mut hud_q: Query<&mut Visibility, With<StairDebugHud>>,
    mut text_q: Query<&mut Text, With<StairDebugHudText>>,
) {
    let Ok(mut vis) = hud_q.single_mut() else {
        return;
    };
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };

    if !panels.stair_debug {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    }
    if *vis != Visibility::Inherited {
        *vis = Visibility::Inherited;
    }

    let want = build_status_text(&snap);
    if **text != want {
        **text = want;
    }
}

/// Build the multi-line status string. Layout:
///
///   === STAIR DEBUG ===
///   drawing : on
///   player  : xz=(+0.00,+0.00)  y=+0.00
///   slope   : up=+0.123  down=-
///
///   counts  green=12  up=4  down=2  gray=1  red=0
///
///   -- orbs (grouped) --
///   green:
///     #01 xz=(+0.10,+0.20) y=+0.05 dy=+0.05
///     ...
///   up-band:
///     ...
fn build_status_text(snap: &StairDebugSnapshot) -> String {
    let mut out = String::with_capacity(2048);

    // Right-hand column: the last two orchestration (resolve_position) verdicts.
    // Each header line is padded to a fixed width, then the orch field for that
    // row is appended, so the wire-height decision reads to the RIGHT and the
    // panel doesn't grow downward.
    const LW: usize = 44; // left-column width before the orch column
    let orch_block = |out: &mut String, i: usize, label: &str| {
        let d = snap.orch[i];
        out.push_str(&format!("{}:\n", label));
        if !d.valid {
            out.push_str("  <none>\n");
            return;
        }
        out.push_str(&format!(
            "  is_a_stop   = {}   why = {}\n",
            d.is_a_stop as u8, d.reason
        ));
        out.push_str(&format!(
            "  block_n     = ({:+.2},{:+.2},{:+.2})\n",
            d.block_nx, d.block_ny, d.block_nz
        ));
        out.push_str(&format!(
            "  hit_pt      = ({:+.1},{:+.1},{:+.1})\n",
            d.hit_x, d.hit_y, d.hit_z
        ));
        out.push_str(&format!(
            "  start_xz    = ({:+.1},{:+.1})\n",
            d.start_x, d.start_z
        ));
        out.push_str(&format!(
            "  stop_slope  = {}   angle = {:.1}\n",
            d.stop_slope as u8, d.slope_angle
        ));
        out.push_str(&format!("  stop_steps  = {}   step_h = {:+.2}  step_slope = {:+.2}  tall_wall_before_step = {}\n", d.stop_steps as u8, d.step_height, d.step_slope, d.tall_wall as u8));
        out.push_str(&format!(
            "  stop_wall   = {}   wall_h = {:+.2}\n",
            d.stop_wall as u8, d.wall_height
        ));
        out.push_str(&format!("  stop_door   = {}\n", d.stop_door as u8));
        out.push_str(&format!(
            "  stop_mob    = {}   soft = {:.2}\n",
            d.stop_mob as u8, d.soft_timer
        ));
    };
    let mut line = |left: String, right: &str| {
        out.push_str(&format!("{:<width$}{}\n", left, right, width = LW));
    };

    line("=== STAIR DEBUG ===".to_string(), "");
    line(
        format!(
            "drawing : {}",
            if snap.drawing_enabled { "on" } else { "off" }
        ),
        "",
    );
    line(
        format!(
            "player  : xz=({:+.2},{:+.2})  y={:+.2}",
            snap.player_xz.x, snap.player_xz.y, snap.player_y,
        ),
        "",
    );
    line(
        format!(
            "zone    : {} (id={})",
            if snap.zone_name.is_empty() {
                "?"
            } else {
                snap.zone_name.as_str()
            },
            snap.zone_id,
        ),
        "",
    );
    line(
        format!(
            "dat     : {}",
            if snap.dat_path.is_empty() {
                "?"
            } else {
                snap.dat_path.as_str()
            },
        ),
        "",
    );
    line(
        format!(
            "slope   : up={}  down={}",
            fmt_opt(snap.slope_up),
            fmt_opt(snap.slope_down)
        ),
        "",
    );

    out.push('\n');
    out.push_str("-- orchestration (last 2 ticks) --\n");
    orch_block(&mut out, 0, "[t-0] newest");
    orch_block(&mut out, 1, "[t-1]");
    if !snap.door_name.is_empty() {
        out.push_str(&format!("DOOR: {}\n", snap.door_name));
    }

    out.push('\n');
    out.push_str(&format!(
        "counts  : green={}  up={}  down={}  gray={}  red={}\n",
        snap.count_green, snap.count_up, snap.count_down, snap.count_gray, snap.count_red,
    ));

    // per-orb dump, grouped by tag so you can scan a category at a glance
    out.push('\n');
    out.push_str("-- orbs (grouped) --\n");

    let live = &snap.orbs[..snap.orb_count.min(snap.orbs.len())];

    push_group(&mut out, "green", live, snap.player_y, |t| {
        matches!(t, OrbTag::Green)
    });
    push_group(&mut out, "up-band", live, snap.player_y, |t| {
        matches!(t, OrbTag::UpBand(_))
    });
    push_group(&mut out, "down-band", live, snap.player_y, |t| {
        matches!(t, OrbTag::DownBand(_))
    });
    push_group(&mut out, "gray", live, snap.player_y, |t| {
        matches!(t, OrbTag::Gray)
    });
    push_group(&mut out, "red", live, snap.player_y, |t| {
        matches!(t, OrbTag::Red)
    });

    // Pin the panel to a CONSTANT line count: the orbs listing varies per tick,
    // and a height that changes every frame makes the panel column reflow --
    // every UI bottom edge below this panel bounces while the camera (and the
    // sample set) moves. Truncate-and-pad so layout never changes.
    const PANEL_LINES: usize = 120;
    let mut lines: Vec<&str> = out.lines().collect();
    lines.truncate(PANEL_LINES);
    let mut fixed = lines.join("\n");
    for _ in lines.len()..PANEL_LINES {
        fixed.push('\n');
    }
    fixed
}

fn push_group(
    out: &mut String,
    label: &str,
    orbs: &[OrbInfo],
    player_y: f32,
    pred: impl Fn(&OrbTag) -> bool,
) {
    let matching: Vec<(usize, &OrbInfo)> = orbs
        .iter()
        .enumerate()
        .filter(|(_, o)| pred(&o.tag))
        .collect();
    if matching.is_empty() {
        return;
    }
    out.push_str(&format!("{}: ({})\n", label, matching.len()));
    for (i, o) in matching {
        out.push_str(&format!(
            "  #{:02} {} xz=({:+.2},{:+.2}) y={:+.2} dy={:+.2}\n",
            i,
            o.tag,
            o.xz.x,
            o.xz.y,
            o.y,
            o.y - player_y,
        ));
    }
}

fn fmt_opt(v: Option<f32>) -> String {
    match v {
        Some(x) => format!("{:+.3}", x),
        None => "-".to_string(),
    }
}

/// Snapshot of stair-debug state, populated by the input crate each frame
/// and consumed by the render crate's status panel. Kept as a plain data
/// resource so the render crate has no dependency on the input crate.
#[derive(Resource, Debug, Clone)]
pub struct StairDebugSnapshot {
    pub drawing_enabled: bool,
    pub player_xz: Vec2,
    pub player_y: f32,
    pub slope_up: Option<f32>,
    pub slope_down: Option<f32>,
    pub count_green: usize,
    pub count_up: usize,
    pub count_down: usize,
    pub count_gray: usize,
    pub count_red: usize,
    pub orb_count: usize,
    pub orbs: [OrbInfo; 60],
    /// The last two resolve_position (orchestration) verdicts, newest first.
    /// Printed as a right-hand column in the debug panel so the wire-height
    /// decision can be watched against what the detector (orbs) sees.
    pub orch: [OrchDecision; 2],
    pub door_name: String,
    /// Zone name (from kuluu_nav::zone_name), or empty if unknown.
    pub zone_name: String,
    /// Live server zone id (SceneState.snapshot.zone_id), 0 if none yet.
    pub zone_id: u16,
    /// Effective MZB DAT path ("ROMx/y/z.DAT"), or empty if unresolved.
    /// Effective = mog-house model wins over zone id (matches
    /// ffxi_dat::zone_dat::effective_zone_dat_file_id).
    pub dat_path: String,
}

/// Shared log of the last two orchestration decisions, written by
/// dispatch_movement_system (input crate) and read into StairDebugSnapshot by
/// the snapshot system. Newest at index 0.
#[derive(Resource, Debug, Clone, Default)]
pub struct OrchDecisionLog {
    pub last_two: [OrchDecision; 2],
    /// Mesh/texture name of the most recent blocking door, for debug.
    pub last_door_name: String,
}

impl OrchDecisionLog {
    /// Push a new decision, shifting the previous into slot 1.
    pub fn push(&mut self, d: OrchDecision) {
        self.last_two[1] = self.last_two[0];
        self.last_two[0] = d;
    }
}

/// One tick's orchestration verdict from resolve_position: what did the wire
/// mover actually decide about the geometry ahead / underfoot.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrchDecision {
    pub valid: bool,
    pub is_a_stop: bool,
    pub stop_slope: bool,
    pub slope_angle: f32,
    pub stop_steps: bool,
    pub tall_wall: bool,
    pub step_slope: f32,
    pub step_height: f32,
    pub stop_wall: bool,
    pub wall_height: f32,
    pub stop_door: bool,
    pub stop_mob: bool,
    pub soft_timer: f32,
    pub block_nx: f32,
    pub block_ny: f32,
    pub block_nz: f32,
    pub reason: &'static str,
    pub hit_x: f32,
    pub hit_y: f32,
    pub hit_z: f32,
    pub start_x: f32,
    pub start_z: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct OrbInfo {
    pub xz: Vec2,
    pub y: f32,
    pub tag: OrbTag,
}

#[derive(Debug, Clone, Copy)]
pub enum OrbTag {
    Green,
    UpBand(i8),
    DownBand(i8),
    Gray,
    Red,
    Empty,
}

impl std::fmt::Display for OrbTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrbTag::Green => write!(f, "green  "),
            OrbTag::UpBand(n) => write!(f, "up+{:<3}", n),
            OrbTag::DownBand(n) => write!(f, "down-{:<2}", n),
            OrbTag::Gray => write!(f, "gray   "),
            OrbTag::Red => write!(f, "red    "),
            OrbTag::Empty => write!(f, "empty  "),
        }
    }
}

impl Default for StairDebugSnapshot {
    fn default() -> Self {
        Self {
            drawing_enabled: true,
            player_xz: Vec2::ZERO,
            player_y: 0.0,
            slope_up: None,
            slope_down: None,
            count_green: 0,
            count_up: 0,
            count_down: 0,
            count_gray: 0,
            count_red: 0,
            orb_count: 0,
            orbs: [OrbInfo {
                xz: Vec2::ZERO,
                y: 0.0,
                tag: OrbTag::Empty,
            }; 60],
            orch: [OrchDecision::default(); 2],
            door_name: String::new(),
            zone_name: String::new(),
            zone_id: 0,
            dat_path: String::new(),
        }
    }
}

/// Marks the stair debug panel's root node so the metrics system can measure
/// its own laid-out rect.
#[derive(bevy::ecs::component::Component)]
pub struct StairDebugPanelRoot;
