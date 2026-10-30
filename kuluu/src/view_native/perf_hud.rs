use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::time::Real;
use kuluu_render::hud::{style::theme, HudPanels};
use kuluu_render::{perf_probe, InGameEntity};

use super::bridge::NativeSource;

const SAMPLES: usize = 120;
const GRAPH_W: f32 = 720.0;
const GRAPH_H: f32 = 64.0;
const BAR_PERCENT: f32 = 100.0 / SAMPLES as f32;
const GRAPH_MAX_MS: f32 = 60.0;
const PERCENTILE_95: f32 = 0.95;
const PERCENTILE_99: f32 = 0.99;
const WARN_BUDGET_RATIO: f32 = 1.25;
const SEVERE_BUDGET_RATIO: f32 = 2.0;
const FONT: f32 = 12.0;
const LOG_COOLDOWN_S: f32 = 1.0;
const SUMMARY_PERIOD_S: f32 = 5.0;
const PANEL_INSET: f32 = 16.0;
const PANEL_MAX_WIDTH_PERCENT: f32 = 95.0;
const PANEL_MAX_HEIGHT_PERCENT: f32 = 80.0;
const SCROLL_LINE_PX: f32 = 24.0;

const GRAPH_BG: Color = Color::srgb(0.08, 0.08, 0.10);

#[derive(Component)]
pub struct PerfPanel;

#[derive(Component)]
pub struct PerfGraphBar(usize);

#[derive(Component)]
pub struct PerfBudgetLine(f32);

#[derive(Component)]
pub struct PerfTextLine(usize);

#[derive(Resource)]
pub struct PerfMonitor {
    samples: [f32; SAMPLES],
    head: usize,

    frame_start: Option<std::time::Instant>,
    last_cpu_us: u64,
    last_main_us: u64,
    thread_cpu_start: Option<std::time::Duration>,
    last_thread_cpu_us: Option<u64>,
    sample_count: usize,
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
    prev_rprep_ns: u64,
    prev_rgraph_ns: u64,
    prev_rtotal_ns: u64,
    prev_rspans_ns: [u64; 5],
    rebuild_rate: f32,

    last_d_model: u64,
    last_d_rasters: u64,
    last_d_rebuilds: u64,
    last_d_probe_us: u64,
    last_d_rprep_us: u64,
    last_d_rgraph_us: u64,
    last_d_rtotal_us: u64,
    last_d_rspans_us: [u64; 5],

    spike_model_loads: u64,
    spike_nameplate_rasters: u64,
    spike_rebuilt: bool,
    spike_rebuild_us: u64,
    spike_probe_us: u64,
    spike_rprep_us: u64,
    spike_rgraph_us: u64,
    spike_rtotal_us: u64,
    spike_rspans_us: [u64; 5],

    prev_churn: [u64; 6],
    last_d_churn: [u64; 6],
    spike_churn: [u64; 6],
    summary_churn: [u64; 6],

    log_cooldown_s: f32,
    suppressed: u32,

    summary_s: f32,
    summary_frames: u32,
    summary_max_ms: f32,
    summary_max_rprep_us: u64,
    summary_max_rgraph_us: u64,
    summary_max_rtotal_us: u64,
    summary_max_rspans_us: [u64; 5],
}

impl Default for PerfMonitor {
    fn default() -> Self {
        Self {
            samples: [0.0; SAMPLES],
            head: 0,
            frame_start: None,
            last_cpu_us: 0,
            last_main_us: 0,
            thread_cpu_start: None,
            last_thread_cpu_us: None,
            sample_count: 0,
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
            prev_rprep_ns: 0,
            prev_rgraph_ns: 0,
            prev_rtotal_ns: 0,
            prev_rspans_ns: [0; 5],
            rebuild_rate: 0.0,
            last_d_model: 0,
            last_d_rasters: 0,
            last_d_rebuilds: 0,
            last_d_probe_us: 0,
            last_d_rprep_us: 0,
            last_d_rgraph_us: 0,
            last_d_rtotal_us: 0,
            last_d_rspans_us: [0; 5],
            spike_model_loads: 0,
            spike_nameplate_rasters: 0,
            spike_rebuilt: false,
            spike_rebuild_us: 0,
            spike_probe_us: 0,
            spike_rprep_us: 0,
            spike_rgraph_us: 0,
            spike_rtotal_us: 0,
            spike_rspans_us: [0; 5],
            prev_churn: [0; 6],
            last_d_churn: [0; 6],
            spike_churn: [0; 6],
            summary_churn: [0; 6],
            log_cooldown_s: 0.0,
            suppressed: 0,
            summary_s: 0.0,
            summary_frames: 0,
            summary_max_ms: 0.0,
            summary_max_rprep_us: 0,
            summary_max_rgraph_us: 0,
            summary_max_rtotal_us: 0,
            summary_max_rspans_us: [0; 5],
        }
    }
}

#[derive(Resource, Default)]
pub struct AssetChurn {
    images: u64,
    image_bytes: u64,
    meshes: u64,
    std_mats: u64,
    zone_mats: u64,
    skinned_mats: u64,
}

pub fn track_asset_churn(
    mut churn: ResMut<AssetChurn>,
    mut img_ev: MessageReader<AssetEvent<Image>>,
    mut mesh_ev: MessageReader<AssetEvent<Mesh>>,
    mut std_ev: MessageReader<AssetEvent<StandardMaterial>>,
    mut zone_ev: MessageReader<AssetEvent<kuluu_render::ffxi_zone_material::FfxiZoneMaterial>>,
    mut skin_ev: MessageReader<
        AssetEvent<kuluu_render::skinned_ffxi_material::FfxiSkinnedMaterial>,
    >,
    images: Res<Assets<Image>>,
) {
    for ev in img_ev.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = ev {
            churn.images += 1;
            if let Some(img) = images.get(*id) {
                churn.image_bytes += if matches!(ev, AssetEvent::Added { .. }) {
                    kuluu_render::gpu_assets::image_bytes(img) as u64
                } else {
                    img.data.as_ref().map_or(0, Vec::len) as u64
                };
            }
        }
    }
    churn.meshes += mesh_ev
        .read()
        .filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. }))
        .count() as u64;
    churn.std_mats += std_ev
        .read()
        .filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. }))
        .count() as u64;
    churn.zone_mats += zone_ev
        .read()
        .filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. }))
        .count() as u64;
    churn.skinned_mats += skin_ev
        .read()
        .filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. }))
        .count() as u64;
}

pub fn mark_frame_start(
    mut m: ResMut<PerfMonitor>,
    panels: Res<HudPanels>,
    _main_thread: bevy::ecs::system::NonSendMarker,
) {
    m.frame_start = Some(std::time::Instant::now());
    m.thread_cpu_start = panels
        .perf
        .then(|| cpu_time::ThreadTime::try_now().ok())
        .flatten()
        .map(|clock| clock.as_duration());
}

pub fn mark_frame_end(mut m: ResMut<PerfMonitor>) {
    if let Some(start) = m.frame_start {
        m.last_cpu_us = start.elapsed().as_micros() as u64;
    }
}

pub fn mark_last_end(mut m: ResMut<PerfMonitor>, _main_thread: bevy::ecs::system::NonSendMarker) {
    if let Some(start) = m.frame_start {
        m.last_main_us = start.elapsed().as_micros() as u64;
    }
    m.last_thread_cpu_us = m.thread_cpu_start.and_then(|start| {
        cpu_time::ThreadTime::try_now()
            .ok()?
            .as_duration()
            .checked_sub(start)
            .map(|elapsed| elapsed.as_micros() as u64)
    });
}

fn top_render_spans(diag: &DiagnosticsStore) -> String {
    let mut spans: Vec<(String, f64)> = diag
        .iter()
        .filter(|d| {
            let path = d.path().as_str();
            path.starts_with("render/")
                && (path.ends_with("/elapsed_cpu") || path.ends_with("/elapsed_gpu"))
        })
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

/// Periodic dump (every 15s) of every `render/*` diagnostic path, gated by
/// `FFXI_GPU_TIMING`. Positive confirmation that GPU spans (e.g. shadow passes) are
/// recording, since spikes are the only other way spans reach the log. Periodic
/// (not one-shot) so the dump captures in-game steady state, not just the
/// launcher scene that's up 15s after startup; `avg` is the steady-state signal
/// over the diagnostic history window, `peak` catches spikes.
pub fn dump_render_diagnostics(
    time: Res<Time<Real>>,
    diag: Res<DiagnosticsStore>,
    mut next_dump: Local<f32>,
) {
    if std::env::var_os("FFXI_GPU_TIMING").is_none() {
        return;
    }
    if *next_dump == 0.0 {
        *next_dump = 15.0;
    }
    if time.elapsed_secs() < *next_dump {
        return;
    }
    *next_dump = time.elapsed_secs() + 15.0;
    let mut n = 0usize;
    for d in diag
        .iter()
        .filter(|d| d.path().as_str().starts_with("render/"))
    {
        let peak = d.values().copied().fold(0.0f64, f64::max);
        let (sum, count) = d
            .values()
            .fold((0.0f64, 0usize), |(s, c), v| (s + v, c + 1));
        let avg = if count > 0 { sum / count as f64 } else { 0.0 };
        info!(target: "perf", "diag {} avg={avg:.2}ms peak={peak:.2}ms", d.path().as_str());
        n += 1;
    }
    info!(target: "perf", "diag dump complete ({n} render/* paths)");
}

pub fn update_perf_monitor(
    time: Res<Time<Real>>,
    source: Res<NativeSource>,
    diag: Res<DiagnosticsStore>,
    churn: Res<AssetChurn>,
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
    m.sample_count = (m.sample_count + 1).min(SAMPLES);
    m.head = (head + 1) % SAMPLES;
    m.secs_since_spike += dt;
    if m.log_cooldown_s > 0.0 {
        m.log_cooldown_s -= dt;
    }

    let model = perf_probe::model_loads();
    let rasters = perf_probe::nameplate_rasters();
    let rebuilds = source.rebuilds_total;
    let probe_ns = perf_probe::debug_probe_ns();
    let rprep_ns = perf_probe::render_prep_ns();
    let rgraph_ns = perf_probe::render_graph_ns();
    let rtotal_ns = perf_probe::render_total_ns();
    let rspans_ns = perf_probe::rprep_spans_ns();
    let d_model = model.wrapping_sub(m.prev_model_loads);
    let d_rasters = rasters.wrapping_sub(m.prev_nameplate_rasters);
    let d_rebuilds = rebuilds.wrapping_sub(m.prev_rebuilds);
    let d_probe_us = probe_ns.wrapping_sub(m.prev_probe_ns) / 1000;
    let d_rprep_us = rprep_ns.wrapping_sub(m.prev_rprep_ns) / 1000;
    let d_rgraph_us = rgraph_ns.wrapping_sub(m.prev_rgraph_ns) / 1000;
    let d_rtotal_us = rtotal_ns.wrapping_sub(m.prev_rtotal_ns) / 1000;
    let mut d_rspans_us = [0u64; 5];
    let mut w_rspans_us = [0u64; 5];
    for i in 0..5 {
        d_rspans_us[i] = rspans_ns[i].wrapping_sub(m.prev_rspans_ns[i]) / 1000;
        w_rspans_us[i] = d_rspans_us[i] + m.last_d_rspans_us[i];
        m.summary_max_rspans_us[i] = m.summary_max_rspans_us[i].max(d_rspans_us[i]);
    }
    m.prev_model_loads = model;
    m.prev_nameplate_rasters = rasters;
    m.prev_rebuilds = rebuilds;
    m.prev_probe_ns = probe_ns;
    m.prev_rprep_ns = rprep_ns;
    m.prev_rgraph_ns = rgraph_ns;
    m.prev_rtotal_ns = rtotal_ns;
    m.prev_rspans_ns = rspans_ns;
    m.last_d_rspans_us = d_rspans_us;
    m.rebuild_rate = m.rebuild_rate * 0.9 + (d_rebuilds as f32 / dt) * 0.1;

    let churn_now = [
        churn.images,
        churn.image_bytes,
        churn.meshes,
        churn.std_mats,
        churn.zone_mats,
        churn.skinned_mats,
    ];
    let mut d_churn = [0u64; 6];
    let mut w_churn = [0u64; 6];
    for i in 0..6 {
        d_churn[i] = churn_now[i].wrapping_sub(m.prev_churn[i]);
        w_churn[i] = d_churn[i] + m.last_d_churn[i];
        m.summary_churn[i] += d_churn[i];
    }
    m.prev_churn = churn_now;
    m.last_d_churn = d_churn;

    m.summary_s += dt;
    m.summary_frames += 1;
    m.summary_max_ms = m.summary_max_ms.max(dt_ms);
    m.summary_max_rprep_us = m.summary_max_rprep_us.max(d_rprep_us);
    m.summary_max_rgraph_us = m.summary_max_rgraph_us.max(d_rgraph_us);
    m.summary_max_rtotal_us = m.summary_max_rtotal_us.max(d_rtotal_us);
    if m.summary_s >= SUMMARY_PERIOD_S {
        let avg_fps = m.summary_frames as f32 / m.summary_s;
        info!(
            target: "perf",
            "summary {avg_fps:.1}fps baseline {:.1}ms max {:.1}ms spikes {} | max rprep {}us {} rgraph {}us rtotal {}us | churn img+{} ({}KB) mesh+{} std+{} zone+{} skin+{}",
            m.baseline_ms,
            m.summary_max_ms,
            m.spikes_total,
            m.summary_max_rprep_us,
            fmt_rprep_spans(&m.summary_max_rspans_us),
            m.summary_max_rgraph_us,
            m.summary_max_rtotal_us,
            m.summary_churn[0],
            m.summary_churn[1] / 1024,
            m.summary_churn[2],
            m.summary_churn[3],
            m.summary_churn[4],
            m.summary_churn[5],
        );
        m.summary_s = 0.0;
        m.summary_frames = 0;
        m.summary_max_ms = 0.0;
        m.summary_max_rprep_us = 0;
        m.summary_max_rgraph_us = 0;
        m.summary_max_rtotal_us = 0;
        m.summary_max_rspans_us = [0; 5];
        m.summary_churn = [0; 6];
    }

    // `Time` delta is sampled at frame start but the counters here are read mid-Update, so a
    // spike's cause can land one frame either side of its measured duration; sum a two-frame
    // window to attribute it regardless of intra-frame system ordering.
    let w_model = d_model + m.last_d_model;
    let w_rasters = d_rasters + m.last_d_rasters;
    let w_rebuilds = d_rebuilds + m.last_d_rebuilds;
    let w_probe_us = d_probe_us + m.last_d_probe_us;
    let w_rprep_us = d_rprep_us + m.last_d_rprep_us;
    let w_rgraph_us = d_rgraph_us + m.last_d_rgraph_us;
    let w_rtotal_us = d_rtotal_us + m.last_d_rtotal_us;
    m.last_d_model = d_model;
    m.last_d_rasters = d_rasters;
    m.last_d_rebuilds = d_rebuilds;
    m.last_d_probe_us = d_probe_us;
    m.last_d_rprep_us = d_rprep_us;
    m.last_d_rgraph_us = d_rgraph_us;
    m.last_d_rtotal_us = d_rtotal_us;

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
    m.spike_rprep_us = w_rprep_us;
    m.spike_rgraph_us = w_rgraph_us;
    m.spike_rtotal_us = w_rtotal_us;
    m.spike_rspans_us = w_rspans_us;
    m.spike_churn = w_churn;
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
        format!("rebuild {}us", m.spike_rebuild_us)
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
        "frame spike {dt_ms:.1}ms (baseline {:.1}ms, +{:.1}ms) after {interval:.2}s \u{2014} cpu {}us late {late_us}us render~{render_us}us [rprep {w_rprep_us}us {} rgraph {w_rgraph_us}us rtotal {w_rtotal_us}us] | {rebuild}, model+{w_model} plate+{w_rasters} probe {w_probe_us}us | churn img+{} ({}KB) mesh+{} std+{} zone+{} skin+{}{extra}{render_spans}",
        m.baseline_ms,
        dt_ms - m.baseline_ms,
        cpu_us,
        fmt_rprep_spans(&w_rspans_us),
        m.spike_churn[0],
        m.spike_churn[1] / 1024,
        m.spike_churn[2],
        m.spike_churn[3],
        m.spike_churn[4],
        m.spike_churn[5],
    );
}

pub fn spawn_perf_hud(mut commands: Commands) {
    commands
        .spawn((
            InGameEntity,
            PerfPanel,
            bevy::ui::RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(96.0),
                left: Val::Px(8.0),
                width: Val::Px(GRAPH_W + PANEL_INSET),
                max_width: Val::Percent(PANEL_MAX_WIDTH_PERCENT),
                max_height: Val::Percent(PANEL_MAX_HEIGHT_PERCENT),
                overflow: Overflow::scroll_y(),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
            GlobalZIndex(10),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            spawn_text_line(p, 0, theme::TITLE);

            p.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(GRAPH_H),
                    min_height: Val::Px(GRAPH_H),
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
                            left: Val::Percent(i as f32 * BAR_PERCENT),
                            bottom: Val::Px(0.0),
                            width: Val::Percent(BAR_PERCENT),
                            height: Val::Px(0.0),
                            ..default()
                        },
                        BackgroundColor(theme::GOOD),
                    ));
                }
                spawn_ref_line(g, 1.0, theme::GOOD);
                spawn_ref_line(g, SEVERE_BUDGET_RATIO, theme::WARN);
            });

            spawn_text_line(p, 1, theme::TEXT);
            spawn_text_line(p, 2, theme::TEXT);
            spawn_text_line(p, 3, theme::MUTED);
            spawn_text_line(p, 4, theme::MUTED);
            spawn_text_line(p, 5, theme::TEXT);
            spawn_text_line(p, 6, theme::TEXT);
            p.spawn((
                super::dev_diagnostics::DevelopmentSummaryText,
                Text::new("Collecting diagnostics..."),
                TextFont {
                    font_size: FONT.into(),
                    ..default()
                },
                TextColor(theme::TEXT),
            ));
        });
}

fn spawn_text_line(p: &mut ChildSpawnerCommands, index: usize, color: Color) {
    p.spawn((
        PerfTextLine(index),
        Text::new(""),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(color),
    ));
}

fn spawn_ref_line(g: &mut ChildSpawnerCommands, ratio: f32, color: Color) {
    g.spawn((
        PerfBudgetLine(ratio),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            bottom: Val::Px(0.0),
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

pub fn scroll_perf_panel(
    panels: Res<HudPanels>,
    mut wheel: MessageReader<MouseWheel>,
    pointer: Option<ResMut<kuluu_render::mouse::MousePointer>>,
    mut query: Query<
        (
            &bevy::ui::RelativeCursorPosition,
            &ComputedNode,
            &mut ScrollPosition,
        ),
        With<PerfPanel>,
    >,
) {
    let delta: f32 = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y * SCROLL_LINE_PX,
            MouseScrollUnit::Pixel => event.y,
        })
        .sum();
    if !panels.perf {
        return;
    }
    let mut pointer = pointer;
    for (cursor, node, mut scroll) in &mut query {
        if cursor.cursor_over() {
            if let Some(pointer) = pointer.as_mut() {
                pointer.wheel = 0.0;
            }
            let max =
                (node.content_size().y - node.size().y).max(0.0) * node.inverse_scale_factor();
            scroll.0.y = (scroll.0.y - delta).clamp(0.0, max);
        }
    }
}

pub fn update_perf_graph(
    panels: Res<HudPanels>,
    m: Res<PerfMonitor>,
    source: Res<NativeSource>,
    diag: Res<DiagnosticsStore>,
    settings: Option<Res<bevy_framepace::FramepaceSettings>>,
    limit: Option<Res<bevy_framepace::FrametimeLimit>>,
    mut bars: Query<(&PerfGraphBar, &mut Node, &mut BackgroundColor)>,
    mut budget_lines: Query<(&PerfBudgetLine, &mut Node, &mut Visibility), Without<PerfGraphBar>>,
    mut lines: Query<(&PerfTextLine, &mut Text, &mut TextColor)>,
) {
    if !panels.perf {
        return;
    }

    let budget_ms = settings
        .as_deref()
        .and_then(|settings| frame_budget_ms(&settings.limiter, limit.as_deref()));
    for (line, mut node, mut visibility) in &mut budget_lines {
        *visibility = if budget_ms.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Some(budget) = budget_ms {
            node.bottom = Val::Px((budget * line.0 / GRAPH_MAX_MS).clamp(0.0, 1.0) * GRAPH_H);
        }
    }
    for (bar, mut node, mut bg) in bars.iter_mut() {
        let ms = m.samples[(m.head + bar.0) % SAMPLES];
        let frac = (ms / GRAPH_MAX_MS).clamp(0.0, 1.0);
        node.height = Val::Px(frac * GRAPH_H);
        bg.0 = sample_color(ms, budget_ms);
    }

    let text = build_text_lines(&m, &source, &diag, budget_ms);
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
    budget_ms: Option<f32>,
) -> Vec<(String, Color)> {
    let fps = diag
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);

    let title = (
        format!(
            "DEVELOPMENT DIAGNOSTICS  {fps:.0} fps  {:.1}ms  (scale {GRAPH_MAX_MS:.0}ms)",
            m.baseline_ms,
        ),
        theme::TITLE,
    );

    let spike = if m.last_spike_ms <= 0.0 {
        ("spike: none yet".to_string(), theme::MUTED)
    } else {
        let recent = m.interval_ema_s > 0.0 && m.secs_since_spike < m.interval_ema_s * 2.0;
        (
            format!(
                "spike {:.0}ms  worst {:.0}ms  ~every {:.2}s  ({:.1}s ago)  n={}",
                m.last_spike_ms, m.worst_ms, m.interval_ema_s, m.secs_since_spike, m.spikes_total,
            ),
            if recent { theme::DANGER } else { theme::MUTED },
        )
    };

    let frame_us = (m.last_spike_ms * 1000.0) as u64;
    let main_us = m.spike_main_us.min(frame_us);
    let split = (
        format!(
            "spike wall time: main {}us  late {}us  remainder~{}us  (rp {} rg {} rt {})",
            m.spike_cpu_us,
            main_us.saturating_sub(m.spike_cpu_us),
            frame_us.saturating_sub(main_us),
            m.spike_rprep_us,
            m.spike_rgraph_us,
            m.spike_rtotal_us,
        ),
        theme::TEXT,
    );

    let rp_split = (
        format!("rp split: {}", fmt_rprep_spans(&m.spike_rspans_us)),
        theme::MUTED,
    );

    let cause = (
        format!(
            "on spike: {}  model+{} plate+{} probe {}us   snap {}us n:{}  {:.0} rebuild/s",
            if m.spike_rebuilt {
                format!("rebuild {}us", m.spike_rebuild_us)
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
        theme::MUTED,
    );

    let mut samples = m.samples[..m.sample_count].to_vec();
    samples.sort_by(f32::total_cmp);
    let intervals = if let Some(budget) = budget_ms {
        let over = samples.iter().filter(|&&ms| ms > budget).count();
        let severe = samples
            .iter()
            .filter(|&&ms| ms > budget * SEVERE_BUDGET_RATIO)
            .count();
        format!(
            "Budget {budget:.2}ms | intervals over budget {over}/{} | over 2x {severe}",
            samples.len()
        )
    } else {
        "Budget unavailable (limiter off or monitor rate unknown)".into()
    };
    let budget = (
        format!(
            "{intervals} | p95 {:.2}ms p99 {:.2}ms",
            percentile(&samples, PERCENTILE_95),
            percentile(&samples, PERCENTILE_99)
        ),
        theme::TEXT,
    );
    let cpu = m
        .last_thread_cpu_us
        .map(|us| format!("{:.2}ms", us as f64 / 1000.0))
        .unwrap_or_else(|| "unavailable".into());
    let timing = (format!("App schedule wall {:.2}ms | main thread CPU {cpu}\nWall includes worker waits and OS scheduling. Use a profiler to identify the cause.", m.last_main_us as f64 / 1000.0), theme::TEXT);
    vec![title, spike, split, rp_split, cause, budget, timing]
}

/// Formats the rprep sub-spans as "[xtr N ast N vws N que N prp N]" (µs).
fn fmt_rprep_spans(us: &[u64; 5]) -> String {
    let mut out = String::from("[");
    for (i, (label, v)) in perf_probe::RPREP_SPAN_LABELS.iter().zip(us).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(label);
        out.push(' ');
        out.push_str(&v.to_string());
    }
    out.push(']');
    out
}

fn frame_budget_ms(
    limiter: &bevy_framepace::Limiter,
    limit: Option<&bevy_framepace::FrametimeLimit>,
) -> Option<f32> {
    let duration = match limiter {
        bevy_framepace::Limiter::Manual(duration) => *duration,
        bevy_framepace::Limiter::Auto => *limit?.0.try_lock().ok()?,
        bevy_framepace::Limiter::Off => return None,
    };
    (!duration.is_zero()).then_some(duration.as_secs_f32() * 1000.0)
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() as f32 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1)]
}

fn sample_color(ms: f32, budget_ms: Option<f32>) -> Color {
    let Some(budget) = budget_ms else {
        return theme::MUTED;
    };
    if ms <= budget * WARN_BUDGET_RATIO {
        theme::GOOD
    } else if ms <= budget * SEVERE_BUDGET_RATIO {
        theme::WARN
    } else {
        theme::DANGER
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::mouse::MousePointer;

    #[test]
    fn budget_uses_the_active_limiter_and_rejects_unknown_rates() {
        use bevy_framepace::Limiter;
        let budget = frame_budget_ms(&Limiter::from_framerate(120.0), None).unwrap();
        assert!((budget - 1000.0 / 120.0).abs() < f32::EPSILON * 100.0);
        assert!(frame_budget_ms(&Limiter::Off, None).is_none());
        assert!(frame_budget_ms(&Limiter::Auto, None).is_none());
        assert!(frame_budget_ms(&Limiter::Auto, Some(&default())).is_none());
    }

    #[test]
    fn percentiles_keep_long_frames_and_ignore_unused_history() {
        assert_eq!(percentile(&[], PERCENTILE_99), 0.0);
        assert_eq!(percentile(&[8.0], PERCENTILE_95), 8.0);
        let mut samples = vec![8.0; 99];
        samples.push(40.0);
        assert_eq!(percentile(&samples, PERCENTILE_95), 8.0);
        assert_eq!(percentile(&samples, 1.0), 40.0);
    }

    #[test]
    fn only_a_visible_hovered_panel_claims_camera_scroll() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseWheel>()
            .init_resource::<HudPanels>()
            .init_resource::<MousePointer>()
            .add_systems(Update, scroll_perf_panel);
        let panel = app
            .world_mut()
            .spawn((
                PerfPanel,
                bevy::ui::RelativeCursorPosition::default(),
                ComputedNode::default(),
                ScrollPosition::default(),
            ))
            .id();
        for (visible, hovered, claimed) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            app.world_mut().resource_mut::<HudPanels>().perf = visible;
            app.world_mut()
                .get_mut::<bevy::ui::RelativeCursorPosition>(panel)
                .unwrap()
                .cursor_over = hovered;
            app.world_mut().resource_mut::<MousePointer>().wheel = 1.0;
            app.world_mut().write_message(MouseWheel {
                unit: MouseScrollUnit::Line,
                x: 0.0,
                y: 1.0,
                phase: bevy::input::touch::TouchPhase::Moved,
                window: Entity::PLACEHOLDER,
            });
            app.update();
            assert_eq!(app.world().resource::<MousePointer>().wheel == 0.0, claimed);
        }
    }
}
