//! 3D operator dashboard — a headless Bevy app rendered into the terminal
//! via `bevy_ratatui_camera`. Subscribes to `SessionState` via a tokio
//! `watch::Receiver` (see the `bridge` module) and dispatches keyboard
//! input as `AgentCommand`s back over an `mpsc::Sender`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bevy::{
    app::ScheduleRunnerPlugin,
    log::LogPlugin,
    prelude::*,
    time::Fixed,
    winit::WinitPlugin,
};
use bevy_ratatui::{RatatuiContext, RatatuiPlugins, kitty::KittyEnabled};
use bevy_ratatui_camera::{RatatuiCameraPlugin, RatatuiCameraWidget};
use ratatui::{
    layout::{Constraint, Direction as LayoutDirection, Layout, Rect},
    style::{Color as TuiColor, Modifier, Style},
    text::Span,
    widgets::{Paragraph, Widget},
};
use tokio::sync::{mpsc, watch};

use ffxi_client::chrome;
use crate::state::{AgentCommand, EntityKind, SessionState};

use scene::{IsSelf, Target, WorldEntity};

pub mod aggro;
pub mod bridge;
pub mod camera;
pub mod floor;
pub mod input;
pub mod scene;

/// Build and run the dashboard. Blocks the calling thread; intended to be
/// called from inside `tokio::task::spawn_blocking` so the tokio runtime
/// keeps draining the session and folder tasks.
pub fn run(
    state_rx: watch::Receiver<SessionState>,
    cmd_tx: mpsc::Sender<AgentCommand>,
    log_rx: mpsc::UnboundedReceiver<String>,
    log_tx: mpsc::UnboundedSender<String>,
    show_all_events: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    App::new()
        .add_plugins((
            DefaultPlugins
                .build()
                .disable::<WinitPlugin>()
                .disable::<LogPlugin>(),
            ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0)),
            RatatuiPlugins::default(),
            RatatuiCameraPlugin,
        ))
        .insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.08)))
        .insert_resource(bridge::SessionStateRx(state_rx))
        .insert_resource(bridge::CommandTx(cmd_tx))
        .insert_resource(bridge::EventLogRx(log_rx))
        .insert_resource(bridge::LogTx(log_tx))
        .insert_resource(bridge::ShowAllEvents(show_all_events))
        .init_resource::<bridge::SessionStateSnapshot>()
        .init_resource::<bridge::EventLog>()
        .init_resource::<input::HeldDirs>()
        .init_resource::<input::KittyHintLogged>()
        .init_resource::<scene::Target>()
        // 20 Hz movement dispatch matches `tui.rs:88` TICK_MS=50, so the
        // wire-side cadence (and per-tick step size) is identical between
        // the 2D TUI and the 3D view. The render loop still runs at 60 Hz.
        .insert_resource(Time::<Fixed>::from_hz(20.0))
        .add_systems(
            Startup,
            (scene::setup_scene, camera::setup_camera, floor::setup_floor),
        )
        .add_systems(
            PreUpdate,
            (
                input::log_kitty_hint_system,
                input::handle_input_system,
                bridge::ingest_state_system,
                bridge::ingest_log_system,
            )
                .chain(),
        )
        .add_systems(FixedUpdate, input::dispatch_movement_system)
        .add_systems(
            Update,
            (
                scene::sync_entities_system,
                // After sync_entities so we have the last word on
                // material assignment for entities that just became
                // aggro on this snapshot tick.
                aggro::sync_aggro_system,
                floor::swap_floor_system,
                camera::chase_camera_system,
                draw_terminal,
            )
                .chain(),
        )
        .run();
    Ok(())
}

/// Compose the full dashboard frame:
///  - top: stage bar (chrome)
///  - body: three columns
///     - left (28 cells): agent state HUD + LLM decision badge
///     - middle (Min 20 cells, fills): 3D camera + entity nametags
///     - right (38 cells): party roster + chat + JSON event log
///  - bottom: diagnostics (chrome)
///
/// Nametags use ratatui text overlays (not 3D-baked text quads) — at
/// halfblock resolution, glyphs rendered into the framebuffer would be
/// unreadable. Projecting world positions through `Camera::world_to_viewport`
/// then mapping pixel coords → terminal cells lets us paint real glyphs
/// directly into the ratatui buffer after the camera widget renders.
///
/// Same pattern applies to all chrome: the camera widget is rendered
/// first, then the chrome paints over the surrounding cells (which were
/// outside the camera region anyway). The left HUD column lives entirely
/// outside the camera area, so it never collides.
fn draw_terminal(
    mut ratatui: ResMut<RatatuiContext>,
    mut camera_widget: Single<&mut RatatuiCameraWidget>,
    snapshot: Res<bridge::SessionStateSnapshot>,
    event_log: Res<bridge::EventLog>,
    show_all: Res<bridge::ShowAllEvents>,
    target: Res<Target>,
    cam_q: Query<(&Camera, &GlobalTransform), (With<Camera3d>, Without<WorldEntity>, Without<IsSelf>)>,
    entity_q: Query<(&WorldEntity, &Transform), Without<IsSelf>>,
    kitty: Option<Res<KittyEnabled>>,
) -> Result {
    let kitty_ok = kitty.is_some();
    let show_all = show_all.0.load(Ordering::Relaxed);
    ratatui.draw(|frame| {
        let chunks = Layout::default()
            .direction(LayoutDirection::Vertical)
            .constraints([
                Constraint::Length(3), // stage bar
                Constraint::Min(8),    // body
                Constraint::Length(3), // diagnostics
            ])
            .split(frame.area());

        chrome::draw_stage_bar(frame, chunks[0], &snapshot.0);

        // Three-column body. Left and right columns are fixed-width so
        // text-dense chrome keeps a consistent footprint at any terminal
        // size; the camera takes the rest. Right column is wider than
        // the left because party rows + JSON event log are wider than
        // the agent HUD's three lines.
        let body = Layout::default()
            .direction(LayoutDirection::Horizontal)
            .constraints([
                Constraint::Length(28), // agent HUD + LLM badge
                Constraint::Min(20),    // 3D camera (fills remainder)
                Constraint::Length(38), // party + chat + event log
            ])
            .split(chunks[1]);

        // Left column: agent HUD on top (5 lines: 3 borders + content),
        // LLM badge below (4 lines). The badge is a stub until V4
        // surfaces real MCP-side timing.
        let left = Layout::default()
            .direction(LayoutDirection::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(4)])
            .split(body[0]);
        chrome::draw_agent_hud(frame, left[0], &snapshot.0);
        chrome::draw_llm_badge(frame, left[1], &snapshot.0.recent_decisions);

        let camera_area = body[1];
        camera_widget.render(camera_area, frame.buffer_mut());
        if let Ok((cam, cam_xform)) = cam_q.single() {
            render_nametags(frame, camera_area, cam, cam_xform, &entity_q, &target);
        }

        // Right column: party roster on top (one row per member, ~9
        // lines max for a full alliance + borders), then chat in the
        // middle, JSON event log on the bottom. Chat gets 50% of the
        // post-party space and the log gets the other 50%.
        let right = Layout::default()
            .direction(LayoutDirection::Vertical)
            .constraints([
                Constraint::Length(8), // party roster (6 rows + borders)
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .split(body[2]);
        chrome::draw_party_roster(frame, right[0], &snapshot.0.party);
        chrome::draw_chat(frame, right[1], &snapshot.0);
        chrome::draw_event_log(frame, right[2], &event_log.lines, show_all);
        chrome::draw_diagnostics(frame, chunks[2], &snapshot.0, None, kitty_ok);
    })?;
    Ok(())
}

/// Project each entity's world position through the camera and paint a
/// short text label one cell above its head. Skips entities behind the
/// camera (`world_to_viewport` returns Err) or with no name yet.
///
/// Pixel-to-cell mapping is the load-bearing detail: `bevy_ratatui_camera`
/// 0.16's autoresize sets the framebuffer to `(area.width * 2,
/// area.height * 4)` pixels (see `camera_readback.rs:372` in that crate),
/// so `world_to_viewport` returns x in `[0..2W]` and y in `[0..4H]`. The
/// strategy downsamples that to one cell per 2×4 pixel block. Earlier
/// math here treated y as `[0..2H]` (assumed halfblocks took 2 px / cell
/// vertically) — that's wrong, the framebuffer is over-sampled. Labels
/// drifted variably down and right because the divisors were off by 2×.
fn render_nametags(
    frame: &mut ratatui::Frame,
    camera_area: Rect,
    cam: &Camera,
    cam_xform: &GlobalTransform,
    entity_q: &Query<(&WorldEntity, &Transform), Without<IsSelf>>,
    target: &Target,
) {
    // Framebuffer dimensions per `bevy_ratatui_camera`'s autoresize.
    let fb_w = (camera_area.width as f32) * 2.0;
    let fb_h = (camera_area.height as f32) * 4.0;

    for (we, t) in entity_q.iter() {
        let Some(name) = we.name.as_deref() else { continue };
        if name.is_empty() {
            continue;
        }
        // Lift the projection point well above the capsule head. The
        // capsule extends from world y=0 to ~1.0; +1.6 puts the label
        // anchor 0.6 units above its top so the row-shift below stays
        // safely above the rendered glyphs even at extreme camera pitch.
        let head_world = t.translation + Vec3::Y * 1.6;
        let Ok(viewport_px) = cam.world_to_viewport(cam_xform, head_world) else {
            continue;
        };
        if viewport_px.x < 0.0
            || viewport_px.x >= fb_w
            || viewport_px.y < 0.0
            || viewport_px.y >= fb_h
        {
            continue;
        }
        let display: String = name.chars().take(10).collect();
        let label_w = display.chars().count() as u16;
        // Pixel → cell. With the 2×4-px-per-cell oversampling, divide x
        // by 2 and y by 4. round() (not as-cast truncation) gives the
        // closest cell, important so half-cell offsets don't always
        // round down (which biased every label one row below the model).
        let cell_x = camera_area.x + (viewport_px.x / 2.0).round() as u16;
        let cell_y_anchor = camera_area.y + (viewport_px.y / 4.0).round() as u16;
        // Label one row above the projected anchor.
        let cell_y = cell_y_anchor.saturating_sub(1).max(camera_area.y);
        let mut start_x = cell_x.saturating_sub(label_w / 2);
        if start_x + label_w > camera_area.x + camera_area.width {
            start_x = (camera_area.x + camera_area.width).saturating_sub(label_w);
        }
        if start_x < camera_area.x {
            start_x = camera_area.x;
        }
        let label_area = Rect::new(start_x, cell_y, label_w, 1);
        let color = if Some(we.id) == target.id {
            TuiColor::Yellow
        } else {
            kind_color(we.kind)
        };
        let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
        frame.render_widget(Paragraph::new(Span::styled(display, style)), label_area);
    }
}

fn kind_color(kind: EntityKind) -> TuiColor {
    match kind {
        EntityKind::Pc => TuiColor::Cyan,
        EntityKind::Npc => TuiColor::White,
        EntityKind::Mob => TuiColor::Red,
        EntityKind::Pet => TuiColor::Yellow,
        EntityKind::Other => TuiColor::DarkGray,
    }
}

