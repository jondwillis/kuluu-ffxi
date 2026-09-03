//! Graphics-debug panel: window/image/panel metrics split out of the stair
//! HUD, plus the rolling panel-position capture (panelpositions.txt) behind
//! its own Debug-menu toggle so the game never spams a log unasked.

use bevy::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::nameplate_final_pass::{NameplateFrameSnap, NAMEPLATE_PASS_DEBUG};

/// Actual swapchain/surface size as the RENDER world sees it, bridged to the
/// main world through atomics (debug-only plumbing). If the user's stale
/// initial-value theory is right, `win=` will move on resize while this
/// stays frozen at the boot size until a fullscreen toggle forces the full
/// reconfigure path.
pub static SURFACE_W: AtomicU32 = AtomicU32::new(0);
pub static SURFACE_H: AtomicU32 = AtomicU32::new(0);

/// RENDER-WORLD system: records the primary window's extracted surface size.
pub fn record_surface_size(windows: Res<bevy::render::view::ExtractedWindows>) {
    if let Some(w) = windows.primary.and_then(|e| windows.windows.get(&e)) {
        SURFACE_W.store(w.physical_width, Ordering::Relaxed);
        SURFACE_H.store(w.physical_height, Ordering::Relaxed);
    }
}

#[derive(Resource, Default)]
pub struct GraphicsDebugState {
    /// Window physical size + scale factor.
    pub win: (u32, u32, f32),
    /// Render-scale off-screen image size (0x0 when the path is inactive).
    pub img: (u32, u32),
    /// The measured panel's laid-out rect: center (physical px) + size.
    pub panel: (f32, f32, f32, f32),
}

#[derive(Component)]
pub struct GraphicsDebugHud;

#[derive(Component)]
pub struct GraphicsDebugText;

pub fn spawn_graphics_debug_hud(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            GraphicsDebugHud,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Px(540.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(crate::hud::style::theme::FRAME_BG),
            BorderColor::all(crate::hud::style::theme::CURSOR),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((GraphicsDebugText, Text::new("")));
        });
}

pub fn update_graphics_debug_hud(
    panels: Res<crate::hud::HudPanels>,
    state: Res<GraphicsDebugState>,
    mut q_root: Query<&mut Visibility, With<GraphicsDebugHud>>,
    mut q_text: Query<&mut Text, With<GraphicsDebugText>>,
) {
    let Ok(mut vis) = q_root.single_mut() else {
        return;
    };
    if !panels.graphics_debug {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    }
    if *vis != Visibility::Inherited {
        *vis = Visibility::Inherited;
    }
    let Ok(mut text) = q_text.single_mut() else {
        return;
    };
    // img: "off" when render-scale isn't producing an off-screen image
    // (Render Scale = 100%), otherwise the target size.
    let img = if state.img.0 == 0 && state.img.1 == 0 {
        "off (Render Scale = 100%)".to_string()
    } else {
        format!("{}x{}", state.img.0, state.img.1)
    };
    let (sw, sh) = (
        SURFACE_W.load(Ordering::Relaxed),
        SURFACE_H.load(Ordering::Relaxed),
    );
    let agree = if sw == state.win.0 && sh == state.win.1 {
        "MATCH"
    } else {
        "MISMATCH"
    };
    let s = format!(
        "=== GRAPHICS DEBUG ===\nwin   : {}x{}  sf={:.3}\nsurf  : {}x{}  [{}]\nimg   : {}\npanel : Party   ({:.2},{:.2}) {:.2}x{:.2}\nposlog: {}",
        state.win.0,
        state.win.1,
        state.win.2,
        sw,
        sh,
        agree,
        img,
        state.panel.0,
        state.panel.1,
        state.panel.2,
        state.panel.3,
        if panels.position_log { "on" } else { "off" },
    );
    if text.0 != s {
        text.0 = s;
    }
}

/// Metrics + optional position log. Measures the STAIR panel's rect (the
/// jitter proxy) via UiGlobalTransform -- the component bevy 0.19 layout
/// actually writes (plain GlobalTransform on UI stays identity). Duplicate
/// tolerant: takes the largest laid-out match. The file capture only runs
/// while the Debug-menu "Position Log" toggle is on; turning it off clears
/// the buffer so a later session starts fresh.
pub fn graphics_debug_metrics_system(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    // Party/self frame (HP/MP/TP/job/Solo). PartyFrameRoot marks each party
    // window's Absolute root node; .iter() below picks the largest, i.e. the
    // populated Party A frame.
    panel: Query<
        (&bevy::ui::ComputedNode, &bevy::ui::UiGlobalTransform),
        With<crate::hud::party_frame::PartyFrameRoot>,
    >,
    panels: Res<crate::hud::HudPanels>,
    mut state: ResMut<GraphicsDebugState>,
    mut history: Local<std::collections::VecDeque<(u64, f32, f32, f32, f32)>>,
    mut frame: Local<u64>,
) {
    if let Ok(w) = windows.single() {
        let p = w.physical_size();
        state.win = (p.x, p.y, w.scale_factor());
    }
    let best = panel
        .iter()
        .map(|(c, t)| (c.size(), t.translation))
        .max_by(|a, b| (a.0.x * a.0.y).total_cmp(&(b.0.x * b.0.y)));
    if let Some((sz, ctr)) = best {
        state.panel = (ctr.x, ctr.y, sz.x, sz.y);
        if panels.position_log {
            *frame += 1;
            history.push_back((*frame, ctr.x, ctr.y, sz.x, sz.y));
            while history.len() > 500 {
                history.pop_front();
            }
            if *frame % 30 == 0 {
                let mut out = String::with_capacity(history.len() * 48 + 40);
                out.push_str("frame\tcenter_x\tcenter_y\tw\th\n");
                for (f, x, y, w, h) in history.iter() {
                    out.push_str(&format!("{f}\t{x:.3}\t{y:.3}\t{w:.3}\t{h:.3}\n"));
                }
                let _ = std::fs::write("panelpositions.txt", out);
            }
        } else if !history.is_empty() {
            history.clear();
            *frame = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Nameplate Debug panel: last two render ticks of the final in-view pass.
// Splits "names don't show" into stages: nothing extracted / hidden culled /
// texture-not-in-GpuImage-cache / bound-but-no-draws / pipeline-compiling /
// drawn-but-off-screen (far_ndc answers that one).
// ---------------------------------------------------------------------------

#[derive(Component)]
pub struct NameplateDebugHud;

#[derive(Component)]
pub struct NameplateDebugText;

/// One tick's pass state, formatted for the panel. Counter chain left to
/// right: plates extracted -> hidden culled upstream -> GPU texture cache
/// misses (nogpuimg) / data-not-uploaded (nodata) -> bound this tick ->
/// drawn into the operator view (pipeline + farthest-plate clip position).
fn nameplate_snap_line(tag: &str, s: NameplateFrameSnap) -> String {
    let draw = if !s.operator_cam {
        "NO OPERATOR CAMERA".to_string()
    } else if !s.reached_draw {
        // Prepare writes bound/gputex unconditionally when it runs; all-zero
        // plus no draw means the render thread simply hadn't finished P/D for
        // this tick at HUD-read time (extract ran ahead of its own pass).
        if s.bound == 0 && s.gpu_images_total == 0 {
            "in flight (P/D not done at read time)".to_string()
        } else {
            "draw stage NOT reached this tick".to_string()
        }
    } else if !s.pipeline_ready && s.target_fmt.is_none() {
        format!(
            "samples={} bound-plates=0 (reached operator view, nothing to draw)",
            s.samples
        )
    } else if !s.pipeline_ready {
        match s.target_fmt {
            Some(f) => format!("pipeline COMPILING fmt={:?} samples={}", f, s.samples),
            None => format!("samples={} draws=0 (no target fmt yet)", s.samples),
        }
    } else if s.draws == 0 {
        match s.target_fmt {
            Some(f) => format!("draws=0 pipe=ok fmt={:?} samples={}", f, s.samples),
            None => format!("draws=0 pipe=ok samples={}", s.samples),
        }
    } else {
        // Main line stays short (panel width): draws + pipeline state only.
        let main = match s.target_fmt {
            Some(f) => format!(
                "draws={} pipe=ok fmt={:?} samples={}",
                s.draws, f, s.samples
            ),
            None => format!("draws={} pipe=ok samples={}", s.draws, s.samples),
        };
        // Farthest plate (head of the blend order): on screen = ndc xy inside
        // [-1, 1] AND w > 0; alpha near 0 means fully faded even if in view.
        let mut tail = format!(
            "far_ndc=({:+.2},{:+.2}) w={:.3} alpha={:.2}",
            s.far_ndc_x, s.far_ndc_y, s.far_w, s.far_alpha
        );
        // Billboard gate breakdown (main-world mirror of Visibility::Hidden):
        // answers "why is `hidden` what it is". Only when there's something to
        // explain.
        if s.bb_total > 0 || s.hidden > 0 {
            tail.push_str(&format!(
                " bb {} visible of {} | self={} depth-gate={} gone={}",
                s.bb_visible, s.bb_total, s.bb_hide_self, s.bb_hidden_depth, s.bb_despawned
            ));
        }
        format!("{}\n      {}", main, tail)
    };
    format!(
        // {tag} captures the `tag` parameter from scope; the bare specifiers
        // take the seven counters in order.
        "{tag}: plates={} hidden={} nogpuimg={} nodata={} bound={} gputex={} | {}",
        s.plates_total, s.hidden, s.no_gpu_image, s.not_had_data, s.bound, s.gpu_images_total, draw
    )
}

pub fn spawn_nameplate_debug_hud(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            NameplateDebugHud,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(240.0),
                left: Val::Px(540.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(crate::hud::style::theme::FRAME_BG),
            BorderColor::all(crate::hud::style::theme::CURSOR),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((NameplateDebugText, Text::new("")));
        });
}

pub fn update_nameplate_debug_hud(
    panels: Res<crate::hud::HudPanels>,
    mut q_root: Query<&mut Visibility, With<NameplateDebugHud>>,
    mut q_text: Query<&mut Text, With<NameplateDebugText>>,
) {
    let Ok(mut vis) = q_root.single_mut() else {
        return;
    };
    if !panels.nameplate_debug {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    }
    if *vis != Visibility::Inherited {
        *vis = Visibility::Inherited;
    }
    let Ok(mut text) = q_text.single_mut() else {
        return;
    };
    // Render-thread-owned ring of the last two ticks; held only for this brief
    // read, never across a system boundary.
    let s = {
        let dbg = NAMEPLATE_PASS_DEBUG
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        format!(
            "=== NAMEPLATE DEBUG (last 2 ticks) ===\n{}\n{}",
            nameplate_snap_line(&format!("f{:>7}", dbg.frame.saturating_sub(1)), dbg.prev),
            nameplate_snap_line(&format!("f{:>7}", dbg.frame), dbg.cur)
        )
    };
    if text.0 != s {
        text.0 = s;
    }
}
