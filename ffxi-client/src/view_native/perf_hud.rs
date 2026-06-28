use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::Real;
use ffxi_viewer_core::hud::{palette, DevHud, HudVerbosity};
use ffxi_viewer_core::{perf_probe, InGameEntity};

use super::bridge::NativeSource;

const SAMPLES: usize = 120;
const GRAPH_W: f32 = 240.0;
const GRAPH_H: f32 = 64.0;
const BAR_W: f32 = GRAPH_W / SAMPLES as f32;
const GRAPH_MAX_MS: f32 = 60.0;
const TARGET_MS: f32 = 1000.0 / 60.0;
const FONT: f32 = 12.0;
const LOG_COOLDOWN_S: f32 = 1.0;

const GRAPH_BG: Color = Color::srgb(0.08, 0.08, 0.10);

#[derive(Component)]
pub struct PerfPanel;

#[derive(Component)]
pub struct PerfGraphBar(usize);

#[derive(Component)]
pub struct PerfTextLine(usize);

#[derive(Resource)]
pub struct PerfMonitor {
    samples: [f32; SAMPLES],
    head: usize,

    seen_first: bool,
    baseline_ms: f32,

    last_spike_ms: f32,
    worst_ms: f32,
    secs_since_spike: f32,
    interval_ema_s: f32,
    spikes_total: u64,

    prev_model_loads: u64,
    prev_nameplate_rasters: u64,
    prev_rebuilds: u64,
    rebuild_rate: f32,

    spike_model_loads: u64,
    spike_nameplate_rasters: u64,
    spike_rebuilt: bool,
    spike_rebuild_us: u64,

    log_cooldown_s: f32,
    suppressed: u32,
}

impl Default for PerfMonitor {
    fn default() -> Self {
        Self {
            samples: [0.0; SAMPLES],
            head: 0,
            seen_first: false,
            baseline_ms: 0.0,
            last_spike_ms: 0.0,
            worst_ms: 0.0,
            secs_since_spike: 0.0,
            interval_ema_s: 0.0,
            spikes_total: 0,
            prev_model_loads: 0,
            prev_nameplate_rasters: 0,
            prev_rebuilds: 0,
            rebuild_rate: 0.0,
            spike_model_loads: 0,
            spike_nameplate_rasters: 0,
            spike_rebuilt: false,
            spike_rebuild_us: 0,
            log_cooldown_s: 0.0,
            suppressed: 0,
        }
    }
}

pub fn update_perf_monitor(
    time: Res<Time<Real>>,
    source: Res<NativeSource>,
    mut m: ResMut<PerfMonitor>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let dt_ms = dt * 1000.0;
    let head = m.head;
    m.samples[head] = dt_ms;
    m.head = (head + 1) % SAMPLES;
    m.secs_since_spike += dt;
    if m.log_cooldown_s > 0.0 {
        m.log_cooldown_s -= dt;
    }

    let model = perf_probe::model_loads();
    let rasters = perf_probe::nameplate_rasters();
    let rebuilds = source.rebuilds_total;
    let d_model = model.wrapping_sub(m.prev_model_loads);
    let d_rasters = rasters.wrapping_sub(m.prev_nameplate_rasters);
    let d_rebuilds = rebuilds.wrapping_sub(m.prev_rebuilds);
    m.prev_model_loads = model;
    m.prev_nameplate_rasters = rasters;
    m.prev_rebuilds = rebuilds;
    m.rebuild_rate = m.rebuild_rate * 0.9 + (d_rebuilds as f32 / dt) * 0.1;

    if !m.seen_first {
        m.baseline_ms = dt_ms;
        m.seen_first = true;
        return;
    }

    let threshold = (m.baseline_ms * 1.4).max(m.baseline_ms + 4.0);
    if dt_ms <= threshold {
        m.baseline_ms = m.baseline_ms * 0.95 + dt_ms * 0.05;
        return;
    }

    let interval = m.secs_since_spike;
    m.spikes_total += 1;
    m.last_spike_ms = dt_ms;
    m.worst_ms = m.worst_ms.max(dt_ms);
    if interval < 10.0 {
        m.interval_ema_s = if m.interval_ema_s == 0.0 {
            interval
        } else {
            m.interval_ema_s * 0.6 + interval * 0.4
        };
    }
    m.spike_model_loads = d_model;
    m.spike_nameplate_rasters = d_rasters;
    m.spike_rebuilt = d_rebuilds > 0;
    m.spike_rebuild_us = source.last_rebuild_us;
    m.secs_since_spike = 0.0;

    if m.log_cooldown_s > 0.0 {
        m.suppressed += 1;
        return;
    }
    let coalesced = m.suppressed;
    m.suppressed = 0;
    m.log_cooldown_s = LOG_COOLDOWN_S;
    let rebuild = if m.spike_rebuilt {
        format!("rebuild {}\u{00b5}s", m.spike_rebuild_us)
    } else {
        "no rebuild".to_string()
    };
    let extra = if coalesced > 0 {
        format!(" (+{coalesced} more in last {LOG_COOLDOWN_S:.0}s)")
    } else {
        String::new()
    };
    warn!(
        target: "perf",
        "frame spike {dt_ms:.1}ms (baseline {:.1}ms, +{:.1}ms) after {interval:.2}s \u{2014} {rebuild}, model+{d_model} plate+{d_rasters}{extra}",
        m.baseline_ms,
        dt_ms - m.baseline_ms,
    );
}

pub fn spawn_perf_hud(mut commands: Commands) {
    commands
        .spawn((
            InGameEntity,
            DevHud,
            PerfPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(96.0),
                left: Val::Px(8.0),
                width: Val::Px(GRAPH_W + 16.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
            GlobalZIndex(10),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            spawn_text_line(p, 0, palette::ACCENT);

            p.spawn((
                Node {
                    width: Val::Px(GRAPH_W),
                    height: Val::Px(GRAPH_H),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(GRAPH_BG),
            ))
            .with_children(|g| {
                for i in 0..SAMPLES {
                    g.spawn((
                        PerfGraphBar(i),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(i as f32 * BAR_W),
                            bottom: Val::Px(0.0),
                            width: Val::Px(BAR_W),
                            height: Val::Px(0.0),
                            ..default()
                        },
                        BackgroundColor(palette::STAGE_GOOD),
                    ));
                }
                spawn_ref_line(g, TARGET_MS, palette::STAGE_GOOD);
                spawn_ref_line(g, TARGET_MS * 2.0, palette::STAGE_TRANSITIONING);
            });

            spawn_text_line(p, 1, palette::TEXT);
            spawn_text_line(p, 2, palette::MUTED);
        });
}

fn spawn_text_line(p: &mut ChildSpawnerCommands, index: usize, color: Color) {
    p.spawn((
        PerfTextLine(index),
        Text::new(""),
        TextFont {
            font_size: FONT,
            ..default()
        },
        TextColor(color),
        TextLayout::new_with_no_wrap(),
    ));
}

fn spawn_ref_line(g: &mut ChildSpawnerCommands, ms: f32, color: Color) {
    g.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            width: Val::Px(GRAPH_W),
            bottom: Val::Px((ms / GRAPH_MAX_MS).clamp(0.0, 1.0) * GRAPH_H),
            height: Val::Px(1.0),
            ..default()
        },
        BackgroundColor(color.with_alpha(0.35)),
    ));
}

pub fn update_perf_graph(
    verbosity: Res<HudVerbosity>,
    m: Res<PerfMonitor>,
    source: Res<NativeSource>,
    diag: Res<DiagnosticsStore>,
    mut bars: Query<(&PerfGraphBar, &mut Node, &mut BackgroundColor)>,
    mut lines: Query<(&PerfTextLine, &mut Text, &mut TextColor)>,
) {
    if !verbosity.dev_hud {
        return;
    }

    for (bar, mut node, mut bg) in bars.iter_mut() {
        let ms = m.samples[(m.head + bar.0) % SAMPLES];
        let frac = (ms / GRAPH_MAX_MS).clamp(0.0, 1.0);
        node.height = Val::Px(frac * GRAPH_H);
        bg.0 = sample_color(ms);
    }

    let text = build_text_lines(&m, &source, &diag);
    for (line, mut t, mut tc) in lines.iter_mut() {
        if let Some((s, c)) = text.get(line.0) {
            if **t != *s {
                **t = s.clone();
            }
            tc.0 = *c;
        }
    }
}

fn build_text_lines(
    m: &PerfMonitor,
    source: &NativeSource,
    diag: &DiagnosticsStore,
) -> Vec<(String, Color)> {
    let fps = diag
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);

    let title = (
        format!(
            "PERF  {fps:.0} fps  {:.1}ms  (scale {GRAPH_MAX_MS:.0}ms)",
            m.baseline_ms,
        ),
        palette::ACCENT,
    );

    let spike = if m.last_spike_ms <= 0.0 {
        ("spike: none yet".to_string(), palette::MUTED)
    } else {
        let recent = m.interval_ema_s > 0.0 && m.secs_since_spike < m.interval_ema_s * 2.0;
        (
            format!(
                "spike {:.0}ms  worst {:.0}ms  ~every {:.2}s  ({:.1}s ago)  n={}",
                m.last_spike_ms, m.worst_ms, m.interval_ema_s, m.secs_since_spike, m.spikes_total,
            ),
            if recent {
                palette::STAGE_BAD
            } else {
                palette::MUTED
            },
        )
    };

    let cause = (
        format!(
            "on spike: {}  model+{} plate+{}   snap {}\u{00b5}s n:{}  {:.0} rebuild/s",
            if m.spike_rebuilt {
                format!("rebuild {}\u{00b5}s", m.spike_rebuild_us)
            } else {
                "no rebuild".to_string()
            },
            m.spike_model_loads,
            m.spike_nameplate_rasters,
            source.last_rebuild_us,
            source.last_entity_count,
            m.rebuild_rate,
        ),
        palette::MUTED,
    );

    vec![title, spike, cause]
}

fn sample_color(ms: f32) -> Color {
    if ms <= TARGET_MS * 1.25 {
        palette::STAGE_GOOD
    } else if ms <= TARGET_MS * 2.0 {
        palette::STAGE_TRANSITIONING
    } else {
        palette::STAGE_BAD
    }
}
