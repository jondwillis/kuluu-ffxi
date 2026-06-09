//! Dynamic local lights from baked over-bright vertices — lanterns,
//! braziers, torches, campfires.
//!
//! # Why this exists
//!
//! Retail FFXI's 2002 engine placed almost no real-time point lights;
//! the warm pool a lantern throws on a wall is **baked into the MMB
//! vertex colours**, and the flame itself is an animated additive
//! sprite. Crucially, FFXI authored those lamp/flame texels with the
//! "0x80 = 1.0" convention pushed past 1.0 — i.e. the glowing parts of
//! a lamp model carry **over-bright vertex colours** (channels > 1.0).
//! We already decode that overbright (see `dat_mmb.rs`), but until now
//! it only fed bloom — nothing actually lit the surroundings, so the
//! world looked flat next to a lantern.
//!
//! # What we do
//!
//! When an MMB submesh spawns ([`MmbOverlay`]), we scan its
//! `ATTRIBUTE_COLOR` for over-bright (and, by default, warm) vertices,
//! cluster them in local space, and spawn an emitter **child** at each
//! cluster. Being a child, the emitter inherits the prop's world
//! transform for free and despawns with it — no coordinate maths, no
//! extra lifecycle drain.
//!
//! Each emitter carries:
//!   * a [`PointLight`] — active only in **Enhanced** sky style (the
//!     modern dynamic look),
//!   * a small additive emissive sphere — the **flame sprite**, shown
//!     in both styles (the faithful retail element),
//! and a flicker that wobbles both each frame.
//!
//! Detection is a heuristic with no ground-truth DAT placement data, so
//! everything is tunable at runtime through [`ZoneLightConfig`] (see the
//! `/lights` slash command) — threshold, intensity, range, caps.

use std::collections::HashMap;

use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::InGameEntity;
use crate::dat_mmb::MmbOverlay;
use crate::graphics_settings::{GraphicsSettings, SkyStyle};

/// Runtime knobs for the dynamic-light heuristic. Persisted nowhere yet
/// (resets each launch); tuned live via `/lights`.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ZoneLightConfig {
    /// Master on/off. When off, no emitters are detected or spawned
    /// (existing ones stay until their prop despawns).
    pub enabled: bool,
    /// A vertex counts as a light source when its brightest channel
    /// exceeds this. FFXI overbright tops out near 2.0 (byte 255 / 128),
    /// so values in `1.1..1.6` pick lamp/flame texels without catching
    /// ordinary bright-white highlights.
    pub overbright_threshold: f32,
    /// Require the over-bright texel to be warm (red ≥ blue) so we light
    /// lanterns and fires, not blue magic crystals or cold skylights.
    pub warm_only: bool,
    /// Max emitters spawned per MMB submesh — guards against a noisy
    /// mesh sprouting dozens of lights.
    pub max_per_mesh: u32,
    /// Hard cap on simultaneously live emitters in a zone.
    pub max_total: u32,
    /// Base [`PointLight`] intensity (lumens) before flicker. Enhanced
    /// style only.
    pub point_intensity: f32,
    /// [`PointLight`] range in metres.
    pub point_range: f32,
    /// Radius of the additive flame sprite sphere.
    pub flame_radius: f32,
    /// Animate intensity/scale with a flame flicker.
    pub flicker: bool,
}

impl Default for ZoneLightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            overbright_threshold: 1.15,
            warm_only: true,
            max_per_mesh: 3,
            max_total: 96,
            point_intensity: 40_000.0,
            point_range: 12.0,
            flame_radius: 0.14,
            flicker: true,
        }
    }
}

/// One detected light source. Intensity/range/scale are read live from
/// [`ZoneLightConfig`] each frame so `/lights` tuning is responsive
/// without re-detecting; only the per-emitter flicker `phase` (so
/// neighbouring flames don't pulse in lockstep) is stored here.
#[derive(Component, Debug, Clone, Copy)]
pub struct ZoneLightEmitter {
    pub phase: f32,
}

/// Cached flame-sprite mesh so we don't rebuild the icosphere per
/// emitter. The material is per-emitter (each carries its own warm
/// tint), but the mesh is shared.
#[derive(Resource)]
struct FlameAssets {
    mesh: Handle<Mesh>,
}

fn init_flame_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mesh = meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap());
    commands.insert_resource(FlameAssets { mesh });
}

/// Scan freshly spawned MMB submeshes for over-bright vertex clusters
/// and spawn an emitter child at each. Runs only on the frame a mesh is
/// added, so the per-frame cost is bounded by how many props spawn that
/// frame.
#[allow(clippy::too_many_arguments)]
fn detect_zone_light_emitters(
    mut commands: Commands,
    cfg: Res<ZoneLightConfig>,
    meshes: Res<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    flame: Option<Res<FlameAssets>>,
    q_new: Query<(Entity, &Mesh3d), Added<MmbOverlay>>,
    q_existing: Query<(), With<ZoneLightEmitter>>,
) {
    if !cfg.enabled {
        return;
    }
    let Some(flame) = flame else {
        return;
    };
    // Current live emitter count — querying the world means this resets
    // automatically when a zone change despawns the old props (and their
    // emitter children), with no manual bookkeeping.
    let mut live = q_existing.iter().count() as u32;
    if live >= cfg.max_total {
        return;
    }

    for (entity, mesh3d) in &q_new {
        if live >= cfg.max_total {
            break;
        }
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            continue;
        };
        let (Some(VertexAttributeValues::Float32x4(colors)), Some(VertexAttributeValues::Float32x3(positions))) = (
            mesh.attribute(Mesh::ATTRIBUTE_COLOR),
            mesh.attribute(Mesh::ATTRIBUTE_POSITION),
        ) else {
            continue;
        };
        if colors.len() != positions.len() {
            continue;
        }

        let clusters = cluster_overbright(colors, positions, &cfg);
        if clusters.is_empty() {
            continue;
        }

        let n = (clusters.len() as u32).min(cfg.max_per_mesh);
        for cluster in clusters.into_iter().take(n as usize) {
            if live >= cfg.max_total {
                break;
            }
            let color = cluster.warm_light_color();
            // Bevy's unlit branch returns `base_color` and ignores
            // `emissive` (same gotcha as the sun disc), so the HDR glow
            // colour must live in `base_color`. With AlphaMode::Add it
            // blends premultiplied-additive over the scene.
            let flame_mat = materials.add(StandardMaterial {
                base_color: (color.to_linear() * 4.0).into(),
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
                    Transform::from_translation(cluster.local).with_scale(Vec3::splat(cfg.flame_radius)),
                    Visibility::Inherited,
                    bevy::light::NotShadowCaster,
                    bevy::light::NotShadowReceiver,
                ));
            });
            live += 1;
        }
    }
}

/// One over-bright cluster in mesh-local space.
struct Cluster {
    local: Vec3,
    color: Vec3,
    /// Sum of per-vertex peak brightness — used to rank clusters so we
    /// keep the brightest when capping per mesh.
    weight: f32,
}

impl Cluster {
    /// Warm, [0,1]-clamped light colour from the cluster's mean vertex
    /// colour. Normalised to its peak so a 2.0-overbright texel doesn't
    /// produce a super-saturated light hue.
    fn warm_light_color(&self) -> Color {
        let peak = self.color.x.max(self.color.y).max(self.color.z).max(1e-3);
        let c = self.color / peak;
        Color::srgb(c.x, c.y, c.z)
    }
}

/// Grid-cluster the over-bright vertices of one mesh. Cell size is
/// coarse (0.75 m) so the many texels of a single lamp collapse to one
/// emitter; returns clusters sorted brightest-first.
fn cluster_overbright(
    colors: &[[f32; 4]],
    positions: &[[f32; 3]],
    cfg: &ZoneLightConfig,
) -> Vec<Cluster> {
    const CELL: f32 = 0.75;
    // (sum_pos, sum_col, sum_weight, count) keyed by quantised cell.
    let mut cells: HashMap<(i32, i32, i32), (Vec3, Vec3, f32, u32)> = HashMap::new();

    for (c, p) in colors.iter().zip(positions.iter()) {
        let peak = c[0].max(c[1]).max(c[2]);
        if peak < cfg.overbright_threshold {
            continue;
        }
        if cfg.warm_only && c[2] > c[0] {
            // Cold texel (blue dominant) — skip when warm_only.
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
    // Brightest clusters first so per-mesh capping keeps the strongest.
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

/// Per-frame: flicker each emitter and gate the [`PointLight`] on the
/// active sky style. The additive flame sprite stays lit in both styles
/// (faithful), but only Enhanced gets a real dynamic [`PointLight`].
fn animate_zone_lights(
    time: Res<Time>,
    cfg: Res<ZoneLightConfig>,
    settings: Res<GraphicsSettings>,
    mut q: Query<(&ZoneLightEmitter, &mut PointLight, &mut Transform)>,
) {
    let enhanced = settings.sky_style == SkyStyle::Enhanced;
    let t = time.elapsed_secs();
    for (emitter, mut light, mut xf) in &mut q {
        // Organic flicker: two detuned sines so it reads as a flame, not
        // a sine pulse. Range ~[0.78, 1.0].
        let flick = if cfg.flicker {
            let a = (t * 11.0 + emitter.phase).sin();
            let b = (t * 23.3 + emitter.phase * 1.7).sin();
            0.89 + 0.08 * a + 0.03 * b
        } else {
            1.0
        };
        // Read intensity/range/scale live from the config so `/lights`
        // tuning takes effect immediately on existing emitters (only the
        // detection threshold needs a zone re-enter).
        light.intensity = if enhanced { cfg.point_intensity * flick } else { 0.0 };
        light.range = cfg.point_range;
        xf.scale = Vec3::splat(cfg.flame_radius * (0.85 + 0.15 * flick));
    }
}

pub struct ZoneLightsPlugin;

impl Plugin for ZoneLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneLightConfig>()
            .add_systems(Startup, init_flame_assets)
            .add_systems(Update, (detect_zone_light_emitters, animate_zone_lights));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_overbright_vertex_clusters() {
        let cfg = ZoneLightConfig::default();
        // Two warm over-bright texels close together + one dim + one
        // cold-bright. Expect a single warm cluster.
        let colors = vec![
            [1.8, 1.2, 0.6, 1.0], // warm bright
            [1.7, 1.1, 0.5, 1.0], // warm bright (same lamp)
            [0.4, 0.4, 0.4, 1.0], // dim — ignored
            [0.3, 0.6, 1.9, 1.0], // cold bright — ignored (warm_only)
        ];
        let positions = vec![
            [10.0, 2.0, 5.0],
            [10.1, 2.0, 5.1],
            [0.0, 0.0, 0.0],
            [-5.0, 1.0, 3.0],
        ];
        let clusters = cluster_overbright(&colors, &positions, &cfg);
        assert_eq!(clusters.len(), 1, "the two warm texels merge, cold/dim drop");
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
        let colors = vec![[1.0, 0.9, 0.8, 1.0]]; // peak 1.0 < 1.15
        let positions = vec![[0.0, 0.0, 0.0]];
        assert!(cluster_overbright(&colors, &positions, &cfg).is_empty());
    }
}
