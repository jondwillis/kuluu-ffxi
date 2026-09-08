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

/// A separate system in the kuluu crate populates StairDebugSnapshot each
/// frame (kuluu-render can't depend on kuluu, so it pulls only this summary).
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

/// Build the multi-line status string (plan §4 step 2 panel spec). ASCII only:
/// this text renders with Bevy's bundled default font, which covers U+0020-7E.
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

    // Header: support + mode/vy + the three heights + gradient/risers/window/
    // speed + the last two ticks' decisions.
    out.push_str(&format!(
        "grounded: {}  speed={:.2}\n",
        if f.grounded { "yes" } else { "no (airborne)" },
        f.speed_yps
    ));
    let vy_txt = if f.vy.abs() > 1e-6 {
        format!("vy={:+.2}", f.vy)
    } else {
        String::new()
    };
    out.push_str(&format!("mode    : {}{}\n", f.mode, vy_txt));
    let d0 = f.decisions[0].as_deref().unwrap_or("-");
    let d1 = f.decisions[1].as_deref().unwrap_or("-");
    out.push_str(&format!("decision: {} -> {}\n", d0, d1));
    out.push_str(&format!(
        "y/h0/tgt: {:+.3} / {} / {}\n",
        snap.player_y,
        fmt_opt_h(f.h0),
        fmt_opt_h(f.target)
    ));
    out.push_str(&format!(
        "g       : along={:+.3} lat={:+.3}\n",
        f.g_along, f.g_lateral
    ));
    out.push_str(&format!(
        "window  : risers={} range={:.3} poof={}\n",
        f.riser_count,
        f.range,
        if f.poof { "yes" } else { "no" }
    ));

    // Sample table: along | lateral | raw | filtered | status.
    out.push_str("along   lat     raw      filt   status\n");
    for s in &f.samples {
        out.push_str(&format!(
            "{:>6.2} {:>5.2} {} {}  {}\n",
            s.along,
            s.lateral,
            fmt_opt_h(s.raw),
            fmt_opt_h(s.filtered),
            s.status
        ));
    }

    // 120-tick strip chart of h0 / target / y: a staircase should show h0 as
    // steps and target + y as one straight line; any zig in y is the bug.
    out.push_str(&format!(
        "last {} ticks ({} cols, .:-=+*#% low->high, o = none):\n",
        f.history.len(),
        STRIP_COLS
    ));
    let ys: Vec<Option<f32>> = f.history.iter().map(|(_, _, y)| Some(*y)).collect();
    let h0s: Vec<Option<f32>> = f.history.iter().map(|(h, _, _)| *h).collect();
    let ts: Vec<Option<f32>> = f.history.iter().map(|(_, t, _)| *t).collect();
    out.push_str(&format!("y      : {}\n", strip_line(&ys)));
    out.push_str(&format!("h0     : {}\n", strip_line(&h0s)));
    out.push_str(&format!("target : {}\n", strip_line(&ts)));

    // Counters: the two numbers step 4's live tests assert on.
    out.push_str(&format!(
        "reversals(120)={}  max|d2y|(120)={:.4}\n",
        f.reversals_120, f.max_d2y_120
    ));
    out
}

/// Height cell for the panel: fixed width so the sample table stays aligned.
fn fmt_opt_h(v: Option<f32>) -> String {
    match v {
        Some(h) => format!("{h:>7.3}"),
        None => "   none ".to_string(),
    }
}

/// Strip-chart columns (panel width ~75 chars at the 11 px default font).
const STRIP_COLS: usize = 64;

/// Sparkline levels, low to high; index 0 is a space.
const STRIP_LEVELS: &[u8] = b" .:-=+*#%";

/// One ASCII sparkline row over `values` (None renders as 'o'). The whole ring
/// shares one scale so the three series can be compared vertically.
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
        // Nearest tick of the ring for this column.
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

/// One ramp-field sample row for the panel's table (plan §4 step 2).
#[derive(Debug, Clone)]
pub struct SampleRow {
    /// Signed distance along the move direction; 0 is under the feet.
    pub along: f32,
    /// Signed lateral offset from the move line (+ left of m).
    pub lateral: f32,
    /// Height as sampled (None for Miss).
    pub raw: Option<f32>,
    /// Height after the lip filter (None when truncated / missed).
    pub filtered: Option<f32>,
    /// `SampleStatus` as a string; the render crate must not depend on the
    /// walker module, so the status crosses the crate boundary as text.
    pub status: String,
}

/// The last tick's ramp field for the panel (plan §4 step 2). All heights are
/// y-up (bevy); wire z grows down.
#[derive(Debug, Clone)]
pub struct FieldSnapshot {
    /// Walk mode after the last tick's vertical pass (`WalkMode` as a string;
    /// the render crate must not depend on the walker module).
    pub mode: String,
    /// Fall velocity (yalms/s, negative = down); 0 when grounded.
    pub vy: f32,
    /// Last two ticks' vertical decisions, oldest first (`VerticalDecision`
    /// as strings; None before the second tick).
    pub decisions: [Option<String>; 2],
    /// Floor under the footprint per the support-probe rule; None airborne.
    pub h0: Option<f32>,
    /// Walking target: envelope when a staircase is in view, else h0 direct.
    pub target: Option<f32>,
    /// Envelope gradient along m (ZERO unless the window is a staircase).
    pub g_along: f32,
    /// Envelope gradient lateral to m.
    pub g_lateral: f32,
    /// 0 = flat/slope, 1 = single step, >= 2 = staircase.
    pub riser_count: u32,
    /// max(h) - min(h) over the window's surviving samples.
    pub range: f32,
    /// Dead band: no ramp, target is h0 direct.
    pub poof: bool,
    /// Support probe grounded flag for this tick.
    pub grounded: bool,
    /// The speed variable (yalms/s) that paced this tick's vertical move.
    pub speed_yps: f32,
    /// Every sample of the last tick (on-line arm + lateral pairs).
    pub samples: Vec<SampleRow>,
    /// Ring oldest-first as (h0, target, y), all y-up; up to 120 ticks.
    pub history: Vec<(Option<f32>, Option<f32>, f32)>,
    /// dy sign reversals over the ring (step 4's live tests assert on this).
    pub reversals_120: u32,
    /// Max |second difference| of y over the ring.
    pub max_d2y_120: f32,
}

/// Snapshot of stair-debug state, populated by the input crate each frame
/// and consumed by the render crate's status panel. Kept as a plain data
/// resource so the render crate has no dependency on the input crate.
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
    /// Effective = mog-house model wins over zone id (matches
    /// ffxi_dat::zone_dat::effective_zone_dat_file_id).
    pub dat_path: String,
    /// Last tick's ramp field; None until dispatch has recorded a tick.
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

/// Marks the stair debug panel's root node so the metrics system can measure
/// its own laid-out rect.
#[derive(bevy::ecs::component::Component)]
pub struct StairDebugPanelRoot;
