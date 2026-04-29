//! Zone floor: a textured ground plane that swaps when the player zones.
//!
//! ## Where the textures come from
//!
//! Vana'diel zone textures aren't shipped with this repo — distribution is
//! a license question we don't try to answer. To use real zone maps,
//! drop PNGs into `ffxi-client/assets/maps/{zone_id}.png` and an optional
//! `assets/maps/zone_meta.json` with per-zone scale/origin (default scale
//! is "image is 200×200 world units centered on zone origin"). Common
//! community sources are Windower's `maps` repository (top-down PNGs) and
//! BG-Wiki SVG exports.
//!
//! ## Real terrain (heightmap) is not implemented
//!
//! True zone elevation lives in client `.DAT` files. If you have a local
//! PlayOnline install and set `FFXI_DAT_PATH=...` we log that we noticed,
//! but parsing/extracting the DAT-side terrain mesh is a follow-up project
//! (the harness plan's Stage-6 "honest constraint" section).

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::bridge::SessionStateSnapshot;

/// Tags the single ground entity so the floor-swap system can find it
/// without scanning every `Mesh3d` in the world.
#[derive(Component)]
pub struct FloorMarker;

/// Tracks which zone the floor is currently displaying so we only swap
/// when the snapshot actually crosses zones — recomputing the material
/// every frame would thrash Bevy's asset system.
#[derive(Resource, Default)]
pub struct FloorState {
    current_zone: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneMetaEntry {
    /// PNG filename relative to `assets/maps/`. Defaults to `{zone_id}.png`
    /// if absent.
    #[serde(default)]
    pub image: Option<String>,
    /// FFXI world coords (x, y) of the texture's center pixel.
    #[serde(default)]
    pub world_origin: [f32; 2],
    /// World units covered by the texture's full width / height.
    #[serde(default = "default_scale")]
    pub scale: [f32; 2],
}

fn default_scale() -> [f32; 2] {
    [200.0, 200.0]
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ZoneMeta(pub HashMap<String, ZoneMetaEntry>);

/// Eagerly read `zone_meta.json` once at startup. If missing, the loader
/// falls back to the default geometry for every zone — operators can drop
/// PNGs into the dir without writing JSON unless they need non-default scale.
#[derive(Resource, Debug, Default)]
pub struct ZoneMetaCache {
    pub by_zone: HashMap<u16, ZoneMetaEntry>,
}

pub fn setup_floor(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Procedural fallback grid lives at startup. The swap system replaces
    // the mesh+material in place if/when a zone-specific PNG is loaded.
    commands.spawn((
        FloorMarker,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(200.0, 200.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.12, 0.10),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    commands.init_resource::<FloorState>();
    commands.insert_resource(load_zone_meta());

    if let Ok(p) = std::env::var("FFXI_DAT_PATH") {
        // Documented seam: future work would mount this and extract real
        // terrain meshes. For now we just acknowledge it.
        info!(
            "FFXI_DAT_PATH={p} detected — terrain mesh upgrade not yet \
             implemented; falling back to PNG floor (and procedural grid \
             where no PNG exists)."
        );
    }
}

/// Watch for zone changes in the snapshot, swap the floor mesh+material
/// when crossings occur. Pure no-op on every frame the zone hasn't
/// changed (which is most of them).
pub fn swap_floor_system(
    asset_server: Res<AssetServer>,
    snapshot: Res<SessionStateSnapshot>,
    mut state: ResMut<FloorState>,
    meta: Res<ZoneMetaCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut floor_q: Query<
        (
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Mesh3d,
            &mut Transform,
        ),
        With<FloorMarker>,
    >,
) {
    let Some(zone_id) = snapshot.0.zone_id else { return };
    if state.current_zone == Some(zone_id) {
        return;
    }
    state.current_zone = Some(zone_id);

    let Ok((mut mat, mut mesh, mut xform)) = floor_q.single_mut() else {
        return;
    };

    // Look for an asset path. zone_meta.json may name it explicitly; else
    // we try `{zone_id}.png` by convention.
    let entry = meta.by_zone.get(&zone_id);
    let image_name = entry
        .and_then(|e| e.image.clone())
        .unwrap_or_else(|| format!("{zone_id}.png"));
    let asset_dir = asset_server_root().join("maps").join(&image_name);

    let (scale, origin) = entry
        .map(|e| (e.scale, e.world_origin))
        .unwrap_or(([200.0, 200.0], [0.0, 0.0]));

    if asset_dir.exists() {
        let handle: Handle<Image> = asset_server.load(format!("maps/{image_name}"));
        *mat = MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(handle),
            perceptual_roughness: 1.0,
            unlit: false,
            ..default()
        }));
        *mesh = Mesh3d(meshes.add(Plane3d::default().mesh().size(scale[0], scale[1])));
        // Origin shift: FFXI x → Bevy x, FFXI y → Bevy -z (per scene::ffxi_to_bevy).
        xform.translation = Vec3::new(origin[0], 0.0, -origin[1]);
        info!(
            zone_id,
            image = %image_name,
            "loaded zone floor texture",
        );
    } else {
        // Procedural fallback: dim grey grid plane, no texture.
        *mat = MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.12, 0.10),
            perceptual_roughness: 1.0,
            ..default()
        }));
        *mesh = Mesh3d(meshes.add(Plane3d::default().mesh().size(200.0, 200.0)));
        xform.translation = Vec3::ZERO;
        debug!(
            zone_id,
            tried = %image_name,
            "no zone texture present; using procedural floor"
        );
    }
}

fn load_zone_meta() -> ZoneMetaCache {
    let path = asset_server_root().join("maps").join("zone_meta.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return ZoneMetaCache::default();
    };
    let Ok(raw) = serde_json::from_slice::<ZoneMeta>(&bytes) else {
        warn!(path = ?path, "zone_meta.json present but failed to parse; ignoring");
        return ZoneMetaCache::default();
    };
    let mut by_zone = HashMap::new();
    for (k, v) in raw.0 {
        if let Ok(id) = k.parse::<u16>() {
            by_zone.insert(id, v);
        }
    }
    ZoneMetaCache { by_zone }
}

/// Bevy's AssetServer reads from `<CARGO_MANIFEST_DIR>/assets/` for binaries.
/// We use the same root for the `std::fs` existence checks above so we don't
/// spawn a Bevy load that immediately fails.
fn asset_server_root() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).join("assets")
}
