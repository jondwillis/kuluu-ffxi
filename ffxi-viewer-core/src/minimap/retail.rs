use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::main_dll::{MainDll, ZoneMapRecord};
use ffxi_dat::map_image::{map_dat_for_zone, parse_graphic, scan_graphics, GraphicFlag};

use crate::snapshot::SceneState;

use super::{MinimapAabb, MinimapState, RetailStatus};

#[derive(Resource, Default)]
pub struct MapCalibration {
    dll: Option<std::sync::Arc<MainDll>>,
    tried: bool,
}

impl MapCalibration {
    pub(crate) fn ensure_dll(&mut self, root: &std::path::Path) -> Option<std::sync::Arc<MainDll>> {
        if !self.tried {
            self.tried = true;
            self.dll = MainDll::load(root).ok().map(std::sync::Arc::new);
        }
        self.dll.clone()
    }
}

#[derive(Resource, Default)]
pub struct PlayerMapGrid {
    pub aabb: Option<MinimapAabb>,
    zone: Option<u16>,
}

const MENUMAP_TEX: f32 = 512.0;

pub(crate) fn zone_map_to_aabb(rec: &ZoneMapRecord) -> MinimapAabb {
    let size = rec.size as f32;
    let off_x = rec.x_offset as f32;
    let off_y = rec.y_offset as f32;
    let min_x = -size * (0.5 - off_x) / MENUMAP_TEX;
    let min_y = size * (0.5 + off_y) / MENUMAP_TEX;
    MinimapAabb {
        min: Vec2::new(min_x, min_y),
        max: Vec2::new(min_x + size, min_y + size),
    }
}

/// Load and decode one zone's map DAT (any floor) into an RGBA image plus its
/// calibrated AABB, without touching the live `MinimapState`. The Map screen's
/// Change Map browser uses this to preview other zones/floors (kuluu-ziru); the
/// live minimap keeps its own event-driven loader below.
pub fn load_zone_map_image(
    dat_root: &ffxi_dat::DatRoot,
    dll: Option<&MainDll>,
    zone: u16,
    idx: u8,
    images: &mut Assets<Image>,
) -> Option<(Handle<Image>, Option<MinimapAabb>)> {
    let file_id = ffxi_dat::map_image::map_dat_for(zone, idx)?;
    let path = dat_root.resolve(file_id).ok()?.path_under(dat_root.root());
    let bytes = std::fs::read(&path).ok()?;
    let graphic = scan_graphics(&bytes).max_by_key(|g| g.width * g.height)?;
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
    let aabb = dll
        .and_then(|d| d.zone_map(zone, idx))
        .map(|rec| zone_map_to_aabb(&rec));
    Some((handle, aabb))
}

#[derive(Resource, Default, Clone)]
pub struct MinimapDatRoot(pub Option<std::sync::Arc<ffxi_dat::DatRoot>>);

#[derive(Message, Debug, Clone, Copy)]
pub struct LoadRetailMapRequest {
    pub file_id: u32,

    pub zone_id: u16,
}

pub struct RetailBackendPlugin;

impl Plugin for RetailBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MinimapDatRoot>()
            .init_resource::<MapCalibration>()
            .init_resource::<PlayerMapGrid>()
            .add_message::<LoadRetailMapRequest>()
            .add_systems(
                Update,
                (
                    auto_load_retail_for_zone_system,
                    process_load_retail_map_requests,
                )
                    .chain(),
            )
            .add_systems(Update, update_player_map_grid);
    }
}

pub fn update_player_map_grid(
    scene_state: Res<SceneState>,
    dat_root: Res<MinimapDatRoot>,
    mut calib: ResMut<MapCalibration>,
    mut grid: ResMut<PlayerMapGrid>,
) {
    let zone = scene_state.snapshot.zone_id;
    if grid.zone == zone {
        return;
    }
    grid.zone = zone;
    grid.aabb = None;

    let Some(zone_id) = zone.filter(|&z| z != 0) else {
        return;
    };
    let Some(root) = dat_root.0.as_ref() else {
        return;
    };
    let Some(dll) = calib.ensure_dll(root.root()) else {
        return;
    };
    grid.aabb = dll.zone_map(zone_id, 0).map(|rec| zone_map_to_aabb(&rec));
}

fn graphic_flags_present(bytes: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut i = 0usize;
    while i + 61 <= bytes.len() {
        let bmi = u32::from_le_bytes([bytes[i + 17], bytes[i + 18], bytes[i + 19], bytes[i + 20]]);
        if bmi == 40 {
            if let Some(gf) = GraphicFlag::from_u8(bytes[i]) {
                let width = i32::from_le_bytes([
                    bytes[i + 21],
                    bytes[i + 22],
                    bytes[i + 23],
                    bytes[i + 24],
                ]);
                let height = i32::from_le_bytes([
                    bytes[i + 25],
                    bytes[i + 26],
                    bytes[i + 27],
                    bytes[i + 28],
                ]);
                let bit_count = u16::from_le_bytes([bytes[i + 31], bytes[i + 32]]);
                let compression = u32::from_le_bytes([
                    bytes[i + 33],
                    bytes[i + 34],
                    bytes[i + 35],
                    bytes[i + 36],
                ]);
                let why = match parse_graphic(&bytes[i..]) {
                    Ok(Some(_)) => "ok".to_string(),
                    Ok(None) => "skipped".to_string(),
                    Err(e) => e.to_string(),
                };
                found.push(format!(
                    "{gf:?}(w={width} h={height} bpp={bit_count} compr={compression}): {why}"
                ));
                if found.len() >= 3 {
                    break;
                }
            }
        }
        i += 1;
    }
    found
}

pub fn auto_load_retail_for_zone_system(
    scene_state: Res<SceneState>,
    mut state: ResMut<MinimapState>,
    mut writer: MessageWriter<LoadRetailMapRequest>,
) {
    let Some(zone_id) = scene_state.snapshot.zone_id else {
        return;
    };
    if zone_id == 0 {
        return;
    }

    // MAP_DAT_TABLE is keyed by zone id, which inside the Mog House still names
    // the surrounding city — there is no retail map for the interior, so drop
    // retail mode and let the TopDown cull-bake re-bake from the MH geometry.
    if scene_state.snapshot.myroom.is_some() {
        if state.retail_image.is_some() {
            state.retail_image = None;
            state.retail_aabb = None;
            state.retail_status =
                RetailStatus::Failed("inside the Mog House (TopDown fallback)".into());
        }
        return;
    }

    if state.retail_image.is_some() && state.retail_zone == Some(zone_id) {
        return;
    }

    if state.retail_image.is_some() {
        state.retail_image = None;
        state.retail_aabb = None;
    }

    if state.retail_failed_zones.contains(&zone_id) {
        return;
    }
    let Some(file_id) = map_dat_for_zone(zone_id) else {
        state.retail_failed_zones.insert(zone_id);
        state.retail_zone = Some(zone_id);
        state.retail_status = RetailStatus::Failed(format!(
            "no map-DAT mapping for zone {zone_id} (not in MAP_DAT_TABLE)"
        ));
        return;
    };
    writer.write(LoadRetailMapRequest { file_id, zone_id });
}

pub fn process_load_retail_map_requests(
    mut events: MessageReader<LoadRetailMapRequest>,
    dat_root: Res<MinimapDatRoot>,
    mut calib: ResMut<MapCalibration>,
    mut state: ResMut<MinimapState>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(dat_root) = dat_root.0.as_ref() else {
        for req in events.read() {
            state.retail_failed_zones.insert(req.zone_id);
            state.retail_zone = Some(req.zone_id);
            state.retail_status =
                RetailStatus::Failed("no DAT root configured (FFXI_DAT_PATH unset?)".into());
        }
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
                state.retail_failed_zones.insert(req.zone_id);
                state.retail_zone = Some(req.zone_id);
                state.retail_status = RetailStatus::Failed(format!(
                    "file_id {} unresolved in DAT tree: {e}",
                    req.file_id
                ));
                continue;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!("minimap/retail: failed to read {}: {}", path.display(), e);
                state.retail_failed_zones.insert(req.zone_id);
                state.retail_zone = Some(req.zone_id);
                state.retail_status =
                    RetailStatus::Failed(format!("read failed: {}: {e}", path.display()));
                continue;
            }
        };

        let Some(graphic) = scan_graphics(&bytes).max_by_key(|g| g.width * g.height) else {
            let flags = graphic_flags_present(&bytes);
            let why = if flags.is_empty() {
                "no Graphic chunk found in DAT".to_string()
            } else {
                format!(
                    "no decodable Graphic chunk; flags present: [{}]",
                    flags.join(", ")
                )
            };
            warn!(
                "minimap/retail: zone {} file {}: {}",
                req.zone_id, req.file_id, why
            );
            state.retail_failed_zones.insert(req.zone_id);
            state.retail_zone = Some(req.zone_id);
            state.retail_status = RetailStatus::Failed(why);
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
            req.zone_id, req.file_id, graphic.width, graphic.height, graphic.category, graphic.id,
        );

        calib.ensure_dll(dat_root.root());

        state.retail_image = Some(handle);
        state.retail_zone = Some(req.zone_id);
        state.retail_status = RetailStatus::Loaded;

        let calibrated = calib
            .dll
            .as_ref()
            .and_then(|dll| dll.zone_map(req.zone_id, 0))
            .map(|rec| zone_map_to_aabb(&rec));
        state.retail_aabb = calibrated.or(state.aabb);
    }
}
