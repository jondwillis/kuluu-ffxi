//! Retail-map minimap backend: loads FFXI's stylized in-game map
//! texture (the `Ctrl+M` bitmap) for the current zone and publishes
//! it on [`super::MinimapState::retail_image`].
//!
//! # Flow
//!
//! 1. Zone change observed → [`auto_load_retail_for_zone_system`]
//!    looks up `ffxi_dat::map_image::map_dat_for_zone(zone_id)` and
//!    queues a [`LoadRetailMapRequest`].
//! 2. [`process_load_retail_map_requests`] reads the DAT bytes via
//!    [`super::DatRootRes`], scans for Graphic chunks, and uploads
//!    the first (largest) one as a Bevy `Image`. Handle lands on
//!    [`super::MinimapState::retail_image`].
//! 3. The UI's `update_minimap_image_source` reactor swaps the
//!    `ImageNode` to the retail texture (when `MinimapMode::Auto` or
//!    `MinimapMode::Retail`).
//!
//! # AABB
//!
//! Retail maps have their own coordinate system — the bitmap covers
//! a specific world XZ range with margin/padding for labels. Without
//! a per-zone AABB table (future work), this loader reuses the
//! [`super::MinimapState::aabb`] (the top-down MZB AABB) as
//! `retail_aabb`. That puts entity dots roughly in the right
//! ballpark but they'll be off-center on most maps because retail
//! crops differently. Per-zone AABB tuning is a follow-up.
//!
//! # AGPL containment
//!
//! Reads from `ffxi_dat::map_image` (the POLUtils-derived parser).
//! No xi-tinkerer linkage — keeps the network-facing `ffxi-mcp`
//! crate's license profile clean.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::map_image::{map_dat_for_zone, scan_graphics};

use crate::snapshot::SceneState;

use super::MinimapState;

/// Front-end-provided handle for resolving DAT file_ids to disk
/// paths. Same shape as `ffxi_client::view_native::DatRootRes`; we
/// declare a viewer-core-local wrapper so this crate doesn't depend
/// on the client crate.
///
/// Front-ends call `insert_resource(MinimapDatRoot(Some(arc)))`
/// before the plugin's auto-load system fires; without the resource,
/// the retail loader silently no-ops (the top-down backend still
/// works).
#[derive(Resource, Default, Clone)]
pub struct MinimapDatRoot(pub Option<std::sync::Arc<ffxi_dat::DatRoot>>);

/// Request to load a specific map-DAT file_id as the retail image
/// for the current zone. Emitted automatically on zone change, or
/// manually via `/minimap loaddat <file_id>`.
#[derive(Message, Debug, Clone, Copy)]
pub struct LoadRetailMapRequest {
    /// DAT file_id to load (POLUtils 5-digit numbering).
    pub file_id: u32,
    /// Zone-id this map belongs to. Stored on `MinimapState` so the
    /// auto-loader can skip duplicate work on the same zone.
    pub zone_id: u16,
}

/// Plugin registration: message, auto-loader, request consumer.
pub struct RetailBackendPlugin;

impl Plugin for RetailBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MinimapDatRoot>()
            .add_message::<LoadRetailMapRequest>()
            .add_systems(
                Update,
                (
                    auto_load_retail_for_zone_system,
                    process_load_retail_map_requests,
                )
                    .chain(),
            );
    }
}

/// Watch `SceneState::snapshot.zone_id`. When it changes (or the
/// retail image isn't loaded yet for the current zone), look up
/// the map-DAT file_id via the compile-time POLUtils table and
/// queue a [`LoadRetailMapRequest`].
///
/// Zones not in the table — anything added after POLUtils froze, or
/// system zones with no in-game map — silently produce no request.
/// The UI then falls back to the top-down bake via `MinimapMode::Auto`.
pub fn auto_load_retail_for_zone_system(
    scene_state: Res<SceneState>,
    state: Res<MinimapState>,
    mut writer: MessageWriter<LoadRetailMapRequest>,
) {
    let Some(zone_id) = scene_state.snapshot.zone_id else {
        return;
    };
    // Already loaded for this zone (retail image present + same zone
    // matches the gate the top-down backend uses for re-bake).
    if state.retail_image.is_some() && state.zone_id == Some(zone_id) {
        return;
    }
    let Some(file_id) = map_dat_for_zone(zone_id) else {
        return;
    };
    writer.write(LoadRetailMapRequest { file_id, zone_id });
}

/// Consume [`LoadRetailMapRequest`]: read the DAT, parse the
/// Graphic chunks, take the largest one, upload as a Bevy `Image`,
/// publish to [`MinimapState`].
///
/// "Largest" because some map DATs ship multiple chunks (legend
/// overlay, labels, etc.) and the actual playable-area map is
/// usually the largest by pixel count. A more refined picker that
/// matches by Graphic.category prefix lands when zones with named
/// floor variants need disambiguation.
pub fn process_load_retail_map_requests(
    mut events: MessageReader<LoadRetailMapRequest>,
    dat_root: Res<MinimapDatRoot>,
    mut state: ResMut<MinimapState>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(dat_root) = dat_root.0.as_ref() else {
        // Front-end never registered a DatRoot — retail backend is
        // a no-op. Drain the events so they don't pile up across
        // ticks.
        for _ in events.read() {}
        return;
    };

    for req in events.read() {
        let path = match dat_root.resolve(req.file_id) {
            Ok(loc) => loc.path_under(dat_root.root()),
            Err(e) => {
                warn!(
                    "minimap/retail: zone {} file_id {} unresolved: {}",
                    req.zone_id, req.file_id, e
                );
                continue;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "minimap/retail: failed to read {}: {}",
                    path.display(),
                    e
                );
                continue;
            }
        };

        // Take the largest graphic by pixel count. FFXI map DATs
        // typically have one big image (the playable map) and
        // sometimes smaller overlay glyphs.
        let Some(graphic) = scan_graphics(&bytes).max_by_key(|g| g.width * g.height) else {
            warn!(
                "minimap/retail: zone {} file {} parsed cleanly but contained no Graphic chunks",
                req.zone_id, req.file_id
            );
            continue;
        };

        let mut image = Image::new(
            Extent3d {
                width: graphic.width,
                height: graphic.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            graphic.rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = bevy::image::ImageSampler::linear();
        let handle = images.add(image);

        info!(
            "minimap/retail: loaded zone {} file {} ({}×{}, category=\"{}\" id=\"{}\")",
            req.zone_id,
            req.file_id,
            graphic.width,
            graphic.height,
            graphic.category,
            graphic.id,
        );

        state.retail_image = Some(handle);
        // No per-zone AABB table yet — reuse the top-down bake's
        // AABB so entity dots are at least within the right zone.
        // Margins/cropping in retail maps will offset the dots; per-
        // zone AABB tuning is a follow-up (likely a sibling vendor
        // scrape against POLUtils' MapXmlPositions equivalent).
        state.retail_aabb = state.aabb;
        // `state.zone_id` is owned by the top-down backend's bake
        // gate; don't overwrite it from here, or the top-down
        // re-bake trigger would never fire on the next zone.
    }
}
