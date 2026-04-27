use std::collections::{HashMap, VecDeque};

use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::InGameEntity;
use crate::dat_mmb::MmbOverlay;
use crate::graphics_settings::GraphicsSettings;

#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ZoneLightConfig {
    pub enabled: bool,

    pub overbright_threshold: f32,

    pub warm_only: bool,

    pub max_per_mesh: u32,

    pub max_total: u32,

    pub point_intensity: f32,

    pub point_range: f32,

    pub flame_radius: f32,

    pub flicker: bool,
}

impl Default for ZoneLightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            overbright_threshold: 1.15,
            warm_only: true,
            max_per_mesh: 3,
            max_total: 48,
            point_intensity: 25_000.0,
            point_range: 8.0,

            flame_radius: 0.06,
            flicker: true,
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct ZoneLightEmitter {
    pub phase: f32,
}

#[derive(Resource)]
struct FlameAssets {
    mesh: Handle<Mesh>,
}

fn init_flame_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mesh = meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap());
    commands.insert_resource(FlameAssets { mesh });
}

#[derive(Resource, Default)]
struct PendingLightScan(VecDeque<Entity>);

const SCAN_BUDGET_PER_FRAME: usize = 24;

fn enqueue_light_scan(
    mut pending: ResMut<PendingLightScan>,
    q_new: Query<Entity, Added<MmbOverlay>>,
) {
    for e in &q_new {
        pending.0.push_back(e);
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_light_scan(
    mut commands: Commands,
    cfg: Res<ZoneLightConfig>,
    settings: Res<GraphicsSettings>,
    meshes: Res<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    flame: Option<Res<FlameAssets>>,
    mut pending: ResMut<PendingLightScan>,
    q_mesh: Query<(&Mesh3d, Option<&crate::hud::mesh_debug::MmbDebugInfo>)>,
    q_existing: Query<(), With<ZoneLightEmitter>>,
) {
    if !cfg.enabled || !settings.sky_embellishments_enabled() {
        pending.0.clear();
        return;
    }
    let Some(flame) = flame else {
        return;
    };

    let mut live = q_existing.iter().count() as u32;
    if live >= cfg.max_total {
        pending.0.clear();
        return;
    }

    let mut processed = 0usize;
    while processed < SCAN_BUDGET_PER_FRAME {
        if live >= cfg.max_total {
            pending.0.clear();
            break;
        }
        let Some(entity) = pending.0.pop_front() else {
            break;
        };
        processed += 1;

        let Ok((mesh3d, debug)) = q_mesh.get(entity) else {
            continue;
        };
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            continue;
        };
        let (
            Some(VertexAttributeValues::Float32x4(colors)),
            Some(VertexAttributeValues::Float32x3(positions)),
        ) = (
            mesh.attribute(Mesh::ATTRIBUTE_COLOR),
            mesh.attribute(Mesh::ATTRIBUTE_POSITION),
        )
        else {
            continue;
        };
        if colors.len() != positions.len() {
            continue;
        }

        let clusters = cluster_overbright(colors, positions, &cfg);
        if clusters.is_empty() {
            continue;
        }

        let (asset, file_id, chunk_idx) = match debug {
            Some(d) => (d.asset_name.as_str(), d.file_id, d.chunk_idx),
            None => ("?", 0u32, 0usize),
        };
        info!(
            "zone_lights: {} cluster(s) on MMB '{}' (file {file_id} chunk {chunk_idx})",
            clusters.len(),
            asset,
        );

        let n = (clusters.len() as u32).min(cfg.max_per_mesh);
        for cluster in clusters.into_iter().take(n as usize) {
            if live >= cfg.max_total {
                break;
            }
            let color = cluster.warm_light_color();

            let flame_mat = materials.add(StandardMaterial {
                base_color: (color.to_linear() * 1.3).into(),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                ..default()
            });
            let phase = cluster.local.x * 1.7 + cluster.local.y * 0.9 + cluster.local.z * 2.3;
            commands.entity(entity).with_children(|c| {
                c.spawn((
                    InGameEntity,
                    ZoneLightEmitter { phase },
                    PointLight {
                        color,
                        intensity: cfg.point_intensity,
                        range: cfg.point_range,
                        radius: 0.05,
                        shadows_enabled: false,
                        ..default()
                    },
                    Mesh3d(flame.mesh.clone()),
                    MeshMaterial3d(flame_mat),
                    Transform::from_translation(cluster.local)
                        .with_scale(Vec3::splat(cfg.flame_radius)),
                    Visibility::Inherited,
                    bevy::light::NotShadowCaster,
                    bevy::light::NotShadowReceiver,
                ));
            });
            live += 1;
        }
    }
}

struct Cluster {
    local: Vec3,
    color: Vec3,

    weight: f32,
}

impl Cluster {
    fn warm_light_color(&self) -> Color {
        let peak = self.color.x.max(self.color.y).max(self.color.z).max(1e-3);
        let c = self.color / peak;
        Color::srgb(c.x, c.y, c.z)
    }
}

fn cluster_overbright(
    colors: &[[f32; 4]],
    positions: &[[f32; 3]],
    cfg: &ZoneLightConfig,
) -> Vec<Cluster> {
    const CELL: f32 = 0.75;

    let mut cells: HashMap<(i32, i32, i32), (Vec3, Vec3, f32, u32)> = HashMap::new();

    for (c, p) in colors.iter().zip(positions.iter()) {
        let peak = c[0].max(c[1]).max(c[2]);
        if peak < cfg.overbright_threshold {
            continue;
        }
        if cfg.warm_only && c[2] > c[0] {
            continue;
        }
        let pos = Vec3::new(p[0], p[1], p[2]);
        let key = (
            (pos.x / CELL).floor() as i32,
            (pos.y / CELL).floor() as i32,
            (pos.z / CELL).floor() as i32,
        );
        let e = cells.entry(key).or_insert((Vec3::ZERO, Vec3::ZERO, 0.0, 0));
        e.0 += pos;
        e.1 += Vec3::new(c[0], c[1], c[2]);
        e.2 += peak;
        e.3 += 1;
    }

    let mut out: Vec<Cluster> = cells
        .into_values()
        .map(|(sum_pos, sum_col, weight, count)| Cluster {
            local: sum_pos / count as f32,
            color: sum_col / count as f32,
            weight,
        })
        .collect();

    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

fn animate_zone_lights(
    time: Res<Time>,
    cfg: Res<ZoneLightConfig>,
    settings: Res<GraphicsSettings>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut q: Query<(
        &ZoneLightEmitter,
        &mut PointLight,
        &mut Transform,
        &mut Visibility,
    )>,
) {
    // `/lights off` sets cfg.enabled = false; honour it here (not just sky
    // embellishments) so toggling off immediately darkens and hides existing
    // emitters instead of leaving them lit until the next zone re-scan. Lamps
    // also follow the Vana'diel dusk/dawn ramp — lit at night, dark by day.
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let night = crate::zone_point_lights::lamp_night_factor(sky.sun_altitude);
    let active = cfg.enabled && settings.sky_embellishments_enabled() && night > 0.0;
    let t = time.elapsed_secs();
    for (emitter, mut light, mut xf, mut vis) in &mut q {
        *vis = if active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        let flick = if cfg.flicker {
            let a = (t * 11.0 + emitter.phase).sin();
            let b = (t * 23.3 + emitter.phase * 1.7).sin();
            0.89 + 0.08 * a + 0.03 * b
        } else {
            1.0
        };

        // Cast intensity is STEADY: the surface feed (ffxi_zone_material.rs) reads
        // this per frame and re-uploads every zone material on change, so a
        // flickering intensity would either storm those uploads or, on the shared
        // instanced-material path, read as scene-wide flicker. The flame flicker
        // lives on the visible blob's size only.
        light.intensity = if active {
            cfg.point_intensity * night
        } else {
            0.0
        };
        light.range = cfg.point_range;
        xf.scale = Vec3::splat(cfg.flame_radius * (0.85 + 0.15 * flick));
    }
}

fn apply_lights_settings(settings: Res<GraphicsSettings>, mut cfg: ResMut<ZoneLightConfig>) {
    if !settings.is_changed() {
        return;
    }
    let enabled = settings.dynamic_lights.enabled();
    let max_total = settings.dynamic_lights.max_total();
    if cfg.enabled != enabled {
        cfg.enabled = enabled;
    }
    if cfg.max_total != max_total {
        cfg.max_total = max_total;
    }
    if (cfg.overbright_threshold - settings.light_threshold).abs() > f32::EPSILON {
        cfg.overbright_threshold = settings.light_threshold;
    }
    if (cfg.point_intensity - settings.light_intensity).abs() > f32::EPSILON {
        cfg.point_intensity = settings.light_intensity;
    }
    if (cfg.point_range - settings.light_range).abs() > f32::EPSILON {
        cfg.point_range = settings.light_range;
    }
    if cfg.flicker != settings.light_flicker {
        cfg.flicker = settings.light_flicker;
    }
}

pub struct ZoneLightsPlugin;

impl Plugin for ZoneLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneLightConfig>()
            .init_resource::<PendingLightScan>()
            .add_systems(Startup, init_flame_assets)
            .add_systems(
                Update,
                (
                    apply_lights_settings,
                    enqueue_light_scan,
                    drain_light_scan,
                    animate_zone_lights,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_overbright_vertex_clusters() {
        let cfg = ZoneLightConfig::default();

        let colors = vec![
            [1.8, 1.2, 0.6, 1.0],
            [1.7, 1.1, 0.5, 1.0],
            [0.4, 0.4, 0.4, 1.0],
            [0.3, 0.6, 1.9, 1.0],
        ];
        let positions = vec![
            [10.0, 2.0, 5.0],
            [10.1, 2.0, 5.1],
            [0.0, 0.0, 0.0],
            [-5.0, 1.0, 3.0],
        ];
        let clusters = cluster_overbright(&colors, &positions, &cfg);
        assert_eq!(
            clusters.len(),
            1,
            "the two warm texels merge, cold/dim drop"
        );
        let c = clusters[0].warm_light_color().to_srgba();
        assert!(c.red >= c.blue, "light colour is warm (r >= b)");
    }

    #[test]
    fn cold_texels_kept_when_warm_only_off() {
        let mut cfg = ZoneLightConfig::default();
        cfg.warm_only = false;
        let colors = vec![[0.3, 0.6, 1.9, 1.0]];
        let positions = vec![[0.0, 0.0, 0.0]];
        assert_eq!(cluster_overbright(&colors, &positions, &cfg).len(), 1);
    }

    #[test]
    fn below_threshold_yields_nothing() {
        let cfg = ZoneLightConfig::default();
        let colors = vec![[1.0, 0.9, 0.8, 1.0]];
        let positions = vec![[0.0, 0.0, 0.0]];
        assert!(cluster_overbright(&colors, &positions, &cfg).is_empty());
    }

    #[test]
    fn graphics_light_defaults_match_config() {
        use crate::graphics_settings as gs;
        let d = ZoneLightConfig::default();
        assert!((d.overbright_threshold - gs::DEFAULT_LIGHT_THRESHOLD).abs() < 1e-6);
        assert!((d.point_intensity - gs::DEFAULT_LIGHT_INTENSITY).abs() < 1e-3);
        assert!((d.point_range - gs::DEFAULT_LIGHT_RANGE).abs() < 1e-6);
        assert_eq!(d.flicker, gs::DEFAULT_LIGHT_FLICKER);
    }
}
