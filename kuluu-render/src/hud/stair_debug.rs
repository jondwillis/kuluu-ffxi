//! Stair-debug HUD: the top-left panel that shows what the walker sees at the
//! player's feet - per probe column along the heading, the standable ground
//! answer, every surface height in the column, and the identity of the wall
//! faces the body chain touches. The producer lives in the kuluu crate
//! (`kuluu/src/view_native/walker/debug.rs`) and fills [`StairDebugSnapshot`]
//! each frame; this panel only renders it behind the `stair_debug` toggle.

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
                width: Val::Px(620.0),
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

/// Build the multi-line status string. ASCII only: this text renders with
/// Bevy's bundled default font, which covers U+0020-7E.
fn build_status_text(snap: &StairDebugSnapshot) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("=== STAIR DEBUG ===\n");
    out.push_str(&format!(
        "drawing : {}\n",
        if snap.drawing_enabled { "on" } else { "off" }
    ));
    out.push_str(&format!(
        "player  : xz=({:+.2},{:+.2})  y={:+.2}\n",
        snap.player_xz.x, snap.player_xz.y, snap.player_y
    ));
    out.push_str(&format!(
        "zone    : {} (id={})\n",
        if snap.zone_name.is_empty() {
            "?"
        } else {
            snap.zone_name.as_str()
        },
        snap.zone_id
    ));
    out.push_str(&format!(
        "dat     : {}\n",
        if snap.dat_path.is_empty() {
            "?"
        } else {
            snap.dat_path.as_str()
        }
    ));

    let Some(f) = &snap.field else {
        out.push_str("field   : (no tick recorded yet)\n");
        return out;
    };

    out.push_str(&format!(
        "grounded: {}  speed={:.2}\n",
        if f.grounded { "yes" } else { "no (airborne)" },
        f.speed_yps
    ));
    out.push_str(&format!("mode    : {}\n", f.mode));
    let d0 = f.decisions[0].as_deref().unwrap_or("-");
    let d1 = f.decisions[1].as_deref().unwrap_or("-");
    out.push_str(&format!("decision: {} -> {}\n", d0, d1));
    let o0 = f.outcomes[0].as_deref().unwrap_or("-");
    let o1 = f.outcomes[1].as_deref().unwrap_or("-");
    out.push_str(&format!("move    : {} -> {}\n", o0, o1));
    out.push_str(&format!(
        "y/h0    : {:+.3} / {}\n",
        snap.player_y,
        fmt_opt_h(f.h0)
    ));
    out.push_str(&format!(
        "meas/ass: {:+.3} / {:.3}\n",
        f.measurement_y, f.assist
    ));

    out.push_str("columns along heading (ground = standable answer at these feet;\n");
    out.push_str("hits = every surface height in the column with its normal.y,\n");
    out.push_str("walls = body-chain wall contacts: d dist n(nx,ny) slot tri sub prop y-range)\n");
    for c in &f.columns {
        let ground_txt = match c.ground {
            Some(h) => format!("{h:+.3}"),
            None => " none ".to_string(),
        };
        out.push_str(&format!(
            "{:>6.2} {:>7}\n    hits : {}\n    walls: {}\n",
            c.along, ground_txt, c.hits, c.walls
        ));
    }

    out.push_str(&format!(
        "last {} ticks ({} cols, .:-=+*#% low->high, o = none):\n",
        f.history.len(),
        STRIP_COLS
    ));
    let ys: Vec<Option<f32>> = f.history.iter().map(|(_, y)| Some(*y)).collect();
    let h0s: Vec<Option<f32>> = f.history.iter().map(|(h, _)| *h).collect();
    out.push_str(&format!("y      : {}\n", strip_line(&ys)));
    out.push_str(&format!("h0     : {}\n", strip_line(&h0s)));

    out.push_str(&format!(
        "reversals(120)={}  max|d2y|(120)={:.4}\n",
        f.reversals_120, f.max_d2y_120
    ));
    out
}

/// Height cell for the panel: fixed width so the rows stay aligned.
fn fmt_opt_h(v: Option<f32>) -> String {
    match v {
        Some(h) => format!("{h:>7.3}"),
        None => "   none ".to_string(),
    }
}

/// Strip-chart columns (panel width ~90 chars at the 11 px default font).
const STRIP_COLS: usize = 64;

/// Sparkline levels, low to high; index 0 is a space.
const STRIP_LEVELS: &[u8] = b" .:-=+*#%";

/// One ASCII sparkline row over `values` (None renders as 'o'). Both series
/// build their own scale here, so compare shapes across rows by eye.
fn strip_line(values: &[Option<f32>]) -> String {
    if values.is_empty() {
        return "(empty)".to_string();
    }
    let present: Vec<f32> = values.iter().filter_map(|v| *v).collect();
    if present.is_empty() {
        return "o".repeat(STRIP_COLS);
    }
    let lo = present.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = present.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let span = (hi - lo).max(1e-6);
    let mut out = String::with_capacity(STRIP_COLS);
    for c in 0..STRIP_COLS {
        let idx =
            ((c as f32 * values.len() as f32 / STRIP_COLS as f32) as usize).min(values.len() - 1);
        match values[idx] {
            None => out.push('o'),
            Some(v) => {
                let level = (((v - lo) / span * (STRIP_LEVELS.len() - 1) as f32).round() as usize)
                    .min(STRIP_LEVELS.len() - 1);
                out.push(STRIP_LEVELS[level] as char);
            }
        }
    }
    out
}

/// One probe column along the heading for the panel's table. All heights are
/// y-up (bevy); wire z grows down. The strings cross the crate boundary pre-
/// formatted because the render crate must not depend on the walker module.
#[derive(Debug, Clone)]
pub struct ColumnRow {
    /// Signed distance along the heading; 0 is under the feet.
    pub along: f32,
    /// Standable ground answer at these feet (`ground_walkable`), None when
    /// nothing standable answers there.
    pub ground: Option<f32>,
    /// Every surface in the column: `height@normal.y`, nearest-first.
    pub hits: String,
    /// Body-chain wall contacts as compact face identities.
    pub walls: String,
}

/// The last tick's probe picture for the panel.
#[derive(Debug, Clone)]
pub struct FieldSnapshot {
    /// Walk mode after the last tick's vertical pass (`WalkMode` as a string).
    pub mode: String,
    /// Last two ticks' vertical decisions, oldest first (None before the
    /// second tick).
    pub decisions: [Option<String>; 2],
    /// Last two ticks' horizontal outcomes, oldest first.
    pub outcomes: [Option<String>; 2],
    /// Measured standable floor under the feet per this tick's contact test;
    /// None when nothing answered inside the follow window.
    pub h0: Option<f32>,
    /// The walker's step-up assist reach for this frame (yalms).
    pub assist: f32,
    /// What `step` published as its measurement feed this tick (yalms,
    /// up-positive): exactly the height change applied to Y.
    pub measurement_y: f32,
    /// This tick's contact-probe grounded flag.
    pub grounded: bool,
    /// The speed variable (yalms/s) that paced this tick.
    pub speed_yps: f32,
    /// Probe columns along the heading, nearest first.
    pub columns: Vec<ColumnRow>,
    /// Ring oldest-first as (h0, y), both y-up; up to 120 ticks.
    pub history: Vec<(Option<f32>, f32)>,
    /// dy sign reversals over the ring.
    pub reversals_120: u32,
    /// Max |second difference| of y over the ring.
    pub max_d2y_120: f32,
}

/// Snapshot of stair-debug state, populated by the kuluu crate each frame and
/// consumed by this panel. Plain data resource: the render crate has no
/// dependency on the input crate.
#[derive(Resource, Debug, Clone)]
pub struct StairDebugSnapshot {
    pub drawing_enabled: bool,
    pub player_xz: Vec2,
    /// Player feet height in y-up (bevy) at snapshot time.
    pub player_y: f32,
    /// Zone name (from kuluu_nav::zone_name), or empty if unknown.
    pub zone_name: String,
    /// Live server zone id (SceneState.snapshot.zone_id), 0 if none yet.
    pub zone_id: u16,
    /// Effective MZB DAT path ("ROMx/y/z.DAT"), or empty if unresolved.
    pub dat_path: String,
    /// Last tick's probe picture; None until dispatch has recorded a tick.
    pub field: Option<FieldSnapshot>,
}

impl Default for StairDebugSnapshot {
    fn default() -> Self {
        Self {
            drawing_enabled: true,
            player_xz: Vec2::ZERO,
            player_y: 0.0,
            zone_name: String::new(),
            zone_id: 0,
            dat_path: String::new(),
            field: None,
        }
    }
}

/// Marks the stair debug panel's root node so layout metrics can find it.
#[derive(bevy::ecs::component::Component)]
pub struct StairDebugPanelRoot;
