use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::Real;
use ffxi_viewer_core::hud::{palette, DevHud, HudVerbosity};
use ffxi_viewer_core::{perf_probe, InGameEntity, OperatorCamera, SceneState, Target};
use ffxi_viewer_wire::EntityKind;

use super::bridge::NativeSource;
use super::input::{build_tab_candidates, TabCycleStack};

const MAX_ROWS: usize = 24;
const INFO_LINES: usize = 4;
const REFRESH_INTERVAL: f32 = 0.2;
const ROW_FONT: f32 = 12.0;
const NAME_WIDTH: usize = 16;

#[derive(Component)]
pub struct TargetListPanel;

#[derive(Component)]
pub struct InfoLine(pub usize);

#[derive(Component)]
pub struct TargetListRow(pub usize);

#[derive(Resource, Default)]
pub struct RebuildRate {
    last_total: u64,
    per_sec: f32,
}

#[derive(Resource)]
pub struct FrameSpikeTracker {
    seen_first: bool,
    baseline_ms: f32,
    last_dt_ms: f32,
    last_spike_ms: f32,
    secs_since_spike: f32,
    interval_ema_s: f32,

    prev_model_loads: u64,
    prev_nameplate_rasters: u64,
    prev_rebuilds: u64,
    model_load_rate: f32,
    nameplate_raster_rate: f32,

    spike_model_loads: u64,
    spike_nameplate_rasters: u64,
    spike_rebuilt: bool,
    spike_rebuild_us: u64,
}

impl Default for FrameSpikeTracker {
    fn default() -> Self {
        Self {
            seen_first: false,
            baseline_ms: 0.0,
            last_dt_ms: 0.0,
            last_spike_ms: 0.0,
            secs_since_spike: 0.0,
            interval_ema_s: 0.0,
            prev_model_loads: 0,
            prev_nameplate_rasters: 0,
            prev_rebuilds: 0,
            model_load_rate: 0.0,
            nameplate_raster_rate: 0.0,
            spike_model_loads: 0,
            spike_nameplate_rasters: 0,
            spike_rebuilt: false,
            spike_rebuild_us: 0,
        }
    }
}

pub fn track_frame_spikes(
    time: Res<Time<Real>>,
    source: Res<NativeSource>,
    mut t: ResMut<FrameSpikeTracker>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let dt_ms = dt * 1000.0;
    t.last_dt_ms = dt_ms;
    t.secs_since_spike += dt;

    let model = perf_probe::model_loads();
    let rasters = perf_probe::nameplate_rasters();
    let rebuilds = source.rebuilds_total;
    let d_model = model.wrapping_sub(t.prev_model_loads);
    let d_rasters = rasters.wrapping_sub(t.prev_nameplate_rasters);
    let d_rebuilds = rebuilds.wrapping_sub(t.prev_rebuilds);
    t.prev_model_loads = model;
    t.prev_nameplate_rasters = rasters;
    t.prev_rebuilds = rebuilds;

    let inv = 1.0 / dt;
    t.model_load_rate = t.model_load_rate * 0.9 + d_model as f32 * inv * 0.1;
    t.nameplate_raster_rate = t.nameplate_raster_rate * 0.9 + d_rasters as f32 * inv * 0.1;

    if !t.seen_first {
        t.baseline_ms = dt_ms;
        t.seen_first = true;
        return;
    }

    let threshold = (t.baseline_ms * 1.4).max(t.baseline_ms + 4.0);
    if dt_ms > threshold {
        t.last_spike_ms = dt_ms;
        if t.secs_since_spike < 10.0 {
            t.interval_ema_s = if t.interval_ema_s == 0.0 {
                t.secs_since_spike
            } else {
                t.interval_ema_s * 0.6 + t.secs_since_spike * 0.4
            };
        }
        t.spike_model_loads = d_model;
        t.spike_nameplate_rasters = d_rasters;
        t.spike_rebuilt = d_rebuilds > 0;
        t.spike_rebuild_us = source.last_rebuild_us;
        t.secs_since_spike = 0.0;
    } else {
        t.baseline_ms = t.baseline_ms * 0.95 + dt_ms * 0.05;
    }
}

pub fn spawn_target_list_hud(mut commands: Commands) {
    commands
        .spawn((
            InGameEntity,
            DevHud,
            TargetListPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(96.0),
                right: Val::Px(8.0),
                width: Val::Px(360.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
            GlobalZIndex(10),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            for i in 0..INFO_LINES {
                p.spawn((
                    InfoLine(i),
                    Text::new(""),
                    TextFont {
                        font_size: ROW_FONT,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                    TextLayout::new_with_no_wrap(),
                ));
            }
            for i in 0..MAX_ROWS {
                p.spawn((
                    TargetListRow(i),
                    Text::new(""),
                    TextFont {
                        font_size: ROW_FONT,
                        ..default()
                    },
                    TextColor(palette::TEXT),
                    TextLayout::new_with_no_wrap(),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ));
            }
        });
}

struct RowData {
    text: String,
    color: Color,
}

#[allow(clippy::too_many_arguments)]
pub fn update_target_list_hud(
    verbosity: Res<HudVerbosity>,
    time: Res<Time>,
    scene: Res<SceneState>,
    target: Res<Target>,
    tab_stack: Res<TabCycleStack>,
    source: Res<NativeSource>,
    spikes: Res<FrameSpikeTracker>,
    frame_diag: Res<DiagnosticsStore>,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut refresh: Local<f32>,
    mut rate: Local<RebuildRate>,
    mut info_q: Query<(&InfoLine, &mut Text, &mut TextColor), Without<TargetListRow>>,
    mut row_q: Query<(&TargetListRow, &mut Text, &mut TextColor, &mut Node), Without<InfoLine>>,
) {
    if !verbosity.dev_hud {
        return;
    }
    *refresh += time.delta_secs();
    if *refresh < REFRESH_INTERVAL {
        return;
    }
    let elapsed = *refresh;
    *refresh = 0.0;

    let Ok((camera, cam_t)) = cam_q.single() else {
        return;
    };
    let cam_global = GlobalTransform::from(*cam_t);
    let snap = &scene.snapshot;

    let party_ids: Vec<u32> = snap.party.iter().map(|p| p.id).collect();
    let owner = snap.self_char_id.unwrap_or(0);
    let owned_pet_ids: Vec<u32> = snap
        .entities
        .iter()
        .filter(|e| matches!(e.kind, EntityKind::Pet) && e.claim_id == owner)
        .map(|e| e.id)
        .collect();

    let order = build_tab_candidates(
        &snap.entities,
        snap.self_pos.pos,
        snap.self_char_id,
        &party_ids,
        &owned_pet_ids,
        |world_pos| camera.world_to_ndc(&cam_global, world_pos),
    );

    let info = build_info_lines(
        &order,
        &tab_stack,
        &source,
        &spikes,
        &frame_diag,
        &mut rate,
        elapsed,
    );
    for (line, mut text, mut color) in info_q.iter_mut() {
        if let Some((s, c)) = info.get(line.0) {
            if **text != *s {
                **text = s.clone();
            }
            color.0 = *c;
        }
    }

    let rows = build_rows(snap, &order, &party_ids, &owned_pet_ids, target.id);
    for (row, mut text, mut color, mut node) in row_q.iter_mut() {
        match rows.get(row.0) {
            Some(data) => {
                if **text != data.text {
                    **text = data.text.clone();
                }
                color.0 = data.color;
                node.display = Display::Flex;
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                    **text = String::new();
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_info_lines(
    order: &[u32],
    tab_stack: &TabCycleStack,
    source: &NativeSource,
    spikes: &FrameSpikeTracker,
    frame_diag: &DiagnosticsStore,
    rate: &mut RebuildRate,
    elapsed: f32,
) -> Vec<(String, Color)> {
    let delta = source.rebuilds_total.wrapping_sub(rate.last_total);
    rate.last_total = source.rebuilds_total;
    let instant = if elapsed > 0.0 {
        delta as f32 / elapsed
    } else {
        0.0
    };
    rate.per_sec = rate.per_sec * 0.6 + instant * 0.4;

    let frame_max = frame_diag
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .map(|d| d.values().copied().fold(0.0f64, f64::max))
        .unwrap_or(0.0);

    let header = (
        format!(
            "TAB CYCLE \u{21bb}  {} on-screen   pending:{}  idle:{:.1}s",
            order.len(),
            tab_stack.pending_len(),
            tab_stack.idle_secs(),
        ),
        palette::ACCENT,
    );

    let perf = (
        format!(
            "snap {}\u{00b5}s n:{}  rebuilds:{:.0}/s  frame {:.1}ms (max {:.1})",
            source.last_rebuild_us,
            source.last_entity_count,
            rate.per_sec,
            spikes.baseline_ms,
            frame_max,
        ),
        palette::MUTED,
    );

    let spike = if spikes.last_spike_ms <= 0.0 {
        ("spike: none yet".to_string(), palette::MUTED)
    } else {
        let recent =
            spikes.interval_ema_s > 0.0 && spikes.secs_since_spike < spikes.interval_ema_s * 2.0;
        (
            format!(
                "spike {:.0}ms  ~every {:.1}s  ({:.1}s ago)",
                spikes.last_spike_ms, spikes.interval_ema_s, spikes.secs_since_spike,
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
            "on spike: rebuild {}  model+{} plate+{}   rates m:{:.1} p:{:.1}/s",
            if spikes.spike_rebuilt {
                format!("{}\u{00b5}s", spikes.spike_rebuild_us)
            } else {
                "no".to_string()
            },
            spikes.spike_model_loads,
            spikes.spike_nameplate_rasters,
            spikes.model_load_rate,
            spikes.nameplate_raster_rate,
        ),
        palette::TEXT,
    );

    vec![header, perf, spike, cause]
}

fn build_rows(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    order: &[u32],
    party_ids: &[u32],
    owned_pet_ids: &[u32],
    current: Option<u32>,
) -> Vec<RowData> {
    let from = snap.self_pos.pos;
    let mut rows: Vec<RowData> = Vec::with_capacity(order.len().min(MAX_ROWS));
    for (idx, &id) in order.iter().enumerate() {
        if idx >= MAX_ROWS {
            break;
        }
        if idx == MAX_ROWS - 1 && order.len() > MAX_ROWS {
            rows.push(RowData {
                text: format!("   \u{2026} +{} more", order.len() - (MAX_ROWS - 1)),
                color: palette::MUTED,
            });
            break;
        }

        let entity = snap.entities.iter().find(|e| e.id == id);
        let name = entity
            .and_then(|e| e.name.clone())
            .unwrap_or_else(|| format!("#{id:08X}"));
        let dist = entity
            .map(|e| {
                let dx = e.pos.x - from.x;
                let dy = e.pos.y - from.y;
                let dz = e.pos.z - from.z;
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
            .unwrap_or(0.0);
        let kind = entity.map(|e| e.kind).unwrap_or(EntityKind::Other);
        let is_party = party_ids.contains(&id) || owned_pet_ids.contains(&id);
        let is_current = current == Some(id);

        let marker = if is_current { '\u{25b6}' } else { '\u{2502}' };
        let tag = if is_party { " \u{2605}" } else { "" };
        let text = format!(
            "{marker}{idx:>2} {} {:>3} {dist:>4.0}y{tag}",
            truncate_pad(&name, NAME_WIDTH),
            kind_label(kind),
        );
        let color = if is_current {
            palette::ACCENT
        } else if is_party {
            palette::STAGE_GOOD
        } else {
            palette::TEXT
        };
        rows.push(RowData { text, color });
    }
    rows
}

fn kind_label(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "PC",
        EntityKind::Npc => "NPC",
        EntityKind::Mob => "MOB",
        EntityKind::Pet => "PET",
        EntityKind::Other => "—",
    }
}

fn truncate_pad(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count > width {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    } else {
        format!("{s:<width$}")
    }
}
