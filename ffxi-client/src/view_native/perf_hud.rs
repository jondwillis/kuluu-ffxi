use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::Real;
use ffxi_viewer_core::hud::{palette, HudPanels};
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

    frame_start: Option<std::time::Instant>,
    last_cpu_us: u64,
    last_main_us: u64,
    spike_cpu_us: u64,
    spike_main_us: u64,

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
    prev_probe_ns: u64,
    rebuild_rate: f32,

    last_d_model: u64,
    last_d_rasters: u64,
    last_d_rebuilds: u64,
    last_d_probe_us: u64,

    spike_model_loads: u64,
    spike_nameplate_rasters: u64,
    spike_rebuilt: bool,
    spike_rebuild_us: u64,
    spike_probe_us: u64,

    log_cooldown_s: f32,
    suppressed: u32,
}

impl Default for PerfMonitor {
    fn default() -> Self {
        Self {
            samples: [0.0; SAMPLES],
            head: 0,
            frame_start: None,
            last_cpu_us: 0,
            last_main_us: 0,
            spike_cpu_us: 0,
            spike_main_us: 0,
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
            prev_probe_ns: 0,
            rebuild_rate: 0.0,
            last_d_model: 0,
            last_d_rasters: 0,
            last_d_rebuilds: 0,
            last_d_probe_us: 0,
            spike_model_loads: 0,
            spike_nameplate_rasters: 0,
            spike_rebuilt: false,
            spike_rebuild_us: 0,
            spike_probe_us: 0,
            log_cooldown_s: 0.0,
            suppressed: 0,
        }
    }
}

pub fn mark_frame_start(mut m: ResMut<PerfMonitor>) {
    m.frame_start = Some(std::time::Instant::now());
}

pub fn mark_frame_end(mut m: ResMut<PerfMonitor>) {
    if let Some(start) = m.frame_start {
        m.last_cpu_us = start.elapsed().as_micros() as u64;
    }
}

pub fn mark_last_end(mut m: ResMut<PerfMonitor>) {
    if let Some(start) = m.frame_start {
        m.last_main_us = start.elapsed().as_micros() as u64;
    }
}

fn top_render_spans(diag: &DiagnosticsStore) -> String {
    let mut spans: Vec<(String, f64)> = diag
        .iter()
        .filter(|d| d.path().as_str().starts_with("render/"))
        .filter_map(|d| {
            let peak = d.values().copied().fold(0.0f64, f64::max);
            (peak > 0.0).then(|| {
                let name = d
                    .path()
                    .as_str()
                    .trim_start_matches("render/")
                    .replace("/elapsed_cpu", "~cpu")
                    .replace("/elapsed_gpu", "~gpu");
                (name, peak)
            })
        })
        .collect();
    spans.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    spans.truncate(3);
    if spans.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = spans.iter().map(|(n, v)| format!("{n} {v:.1}ms")).collect();
    format!("  peak render: {}", parts.join(", "))
}

pub fn update_perf_monitor(
    time: Res<Time<Real>>,
    source: Res<NativeSource>,
    diag: Res<DiagnosticsStore>,
    mut m: ResMut<PerfMonitor>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let dt_ms = dt * 1000.0;
    let cpu_us = m.last_cpu_us;
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
    let probe_ns = perf_probe::debug_probe_ns();
    let d_model = model.wrapping_sub(m.prev_model_loads);
    let d_rasters = rasters.wrapping_sub(m.prev_nameplate_rasters);
    let d_rebuilds = rebuilds.wrapping_sub(m.prev_rebuilds);
    let d_probe_us = probe_ns.wrapping_sub(m.prev_probe_ns) / 1000;
    m.prev_model_loads = model;
    m.prev_nameplate_rasters = rasters;
    m.prev_rebuilds = rebuilds;
    m.prev_probe_ns = probe_ns;
    m.rebuild_rate = m.rebuild_rate * 0.9 + (d_rebuilds as f32 / dt) * 0.1;

    // `Time` delta is sampled at frame start but the counters here are read mid-Update, so a
    // spike's cause can land one frame either side of its measured duration; sum a two-frame
    // window to attribute it regardless of intra-frame system ordering.
    let w_model = d_model + m.last_d_model;
    let w_rasters = d_rasters + m.last_d_rasters;
    let w_rebuilds = d_rebuilds + m.last_d_rebuilds;
    let w_probe_us = d_probe_us + m.last_d_probe_us;
    m.last_d_model = d_model;
    m.last_d_rasters = d_rasters;
    m.last_d_rebuilds = d_rebuilds;
    m.last_d_probe_us = d_probe_us;

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
    m.spike_model_loads = w_model;
    m.spike_nameplate_rasters = w_rasters;
    m.spike_rebuilt = w_rebuilds > 0;
    m.spike_rebuild_us = source.last_rebuild_us;
    m.spike_probe_us = w_probe_us;
    m.spike_cpu_us = cpu_us;
    m.spike_main_us = m.last_main_us;
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
    let frame_us = (dt_ms * 1000.0) as u64;
    let main_us = m.spike_main_us.min(frame_us);
    let late_us = main_us.saturating_sub(cpu_us);
    let render_us = frame_us.saturating_sub(main_us);
    let render_spans = top_render_spans(&diag);
    warn!(
        target: "perf",
        "frame spike {dt_ms:.1}ms (baseline {:.1}ms, +{:.1}ms) after {interval:.2}s \u{2014} cpu {}\u{00b5}s late {late_us}\u{00b5}s render~{render_us}\u{00b5}s | {rebuild}, model+{w_model} plate+{w_rasters} probe {w_probe_us}\u{00b5}s{extra}{render_spans}",
        m.baseline_ms,
        dt_ms - m.baseline_ms,
        cpu_us,
    );
}

pub fn spawn_perf_hud(mut commands: Commands) {
    commands
        .spawn((
            InGameEntity,
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
            spawn_text_line(p, 2, palette::TEXT);
            spawn_text_line(p, 3, palette::MUTED);
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

pub fn apply_perf_visibility(
    panels: Res<HudPanels>,
    mut q: Query<&mut Visibility, With<PerfPanel>>,
) {
    if !panels.is_changed() {
        return;
    }
    let want = if panels.perf {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in q.iter_mut() {
        if *v != want {
            *v = want;
        }
    }
}

pub fn update_perf_graph(
    panels: Res<HudPanels>,
    m: Res<PerfMonitor>,
    source: Res<NativeSource>,
    diag: Res<DiagnosticsStore>,
    mut bars: Query<(&PerfGraphBar, &mut Node, &mut BackgroundColor)>,
    mut lines: Query<(&PerfTextLine, &mut Text, &mut TextColor)>,
) {
    if !panels.perf {
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

    let frame_us = (m.last_spike_ms * 1000.0) as u64;
    let main_us = m.spike_main_us.min(frame_us);
    let split = (
        format!(
            "spike split: cpu {}\u{00b5}s  late {}\u{00b5}s  render~{}\u{00b5}s",
            m.spike_cpu_us,
            main_us.saturating_sub(m.spike_cpu_us),
            frame_us.saturating_sub(main_us),
        ),
        palette::TEXT,
    );

    let cause = (
        format!(
            "on spike: {}  model+{} plate+{} probe {}\u{00b5}s   snap {}\u{00b5}s n:{}  {:.0} rebuild/s",
            if m.spike_rebuilt {
                format!("rebuild {}\u{00b5}s", m.spike_rebuild_us)
            } else {
                "no rebuild".to_string()
            },
            m.spike_model_loads,
            m.spike_nameplate_rasters,
            m.spike_probe_us,
            source.last_rebuild_us,
            source.last_entity_count,
            m.rebuild_rate,
        ),
        palette::MUTED,
    );

    vec![title, spike, split, cause]
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
