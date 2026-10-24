//! Warming the backdrop zones the character list might flip to, so a flip is not
//! a load. Only ever registered during `AppPhase::Launcher`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};

use kuluu_render::dat_mmb::{self, LoadedMmb, MmbParseCache, MmbTexPools};
use kuluu_render::dat_mzb::{LoadedZoneGeom, ZoneGeomCache, ZONE_GEOM_CACHE_CAP};
use kuluu_render::dat_root::SharedDatRoot;
use kuluu_render::zone_texture::TextureQuality;

/// Candidate zones tracked at once, under `ZONE_GEOM_CACHE_CAP` so the blocks a
/// flip moves between cannot evict each other.
const PRELOAD_MAX_ZONES: usize = ZONE_GEOM_CACHE_CAP.saturating_sub(1);

const PRELOAD_MAX_PARSE_INFLIGHT: usize = 8;

/// Main-thread allowance per frame. Cache inserts and texture uploads happen
/// here, so warming has to stay inside a slice whatever the zone count.
const PRELOAD_SLICE_MS: u128 = 2;

pub fn zone_geom_key(file_id: u32) -> (u32, Option<usize>, Option<u32>) {
    (file_id, None, None)
}

#[derive(Resource, Default)]
pub struct BackdropPreloads {
    targets: Vec<u32>,
    warming_geom: Option<(u32, Task<LoadedZoneGeom>)>,
    /// Chunk indices still to parse for a zone whose geometry is resident.
    pending_models: HashMap<u32, VecDeque<usize>>,
    parsing: HashMap<(u32, usize), Task<Option<LoadedMmb>>>,
    ready: HashSet<u32>,
}

impl BackdropPreloads {
    /// The zones worth warming, in the order the cursor is likely to reach them.
    /// The zone already on screen is left out by the caller; a dropped target
    /// loses its progress rather than keeping it half-done.
    pub fn set_targets(&mut self, targets: Vec<u32>) {
        let mut targets = targets;
        targets.sort_unstable();
        targets.dedup();
        targets.truncate(PRELOAD_MAX_ZONES);
        if targets == self.targets {
            return;
        }
        let kept = |file_id: &u32| targets.contains(file_id);
        self.pending_models.retain(|file_id, _| kept(file_id));
        self.parsing.retain(|(file_id, _), _| kept(file_id));
        if self
            .warming_geom
            .as_ref()
            .is_some_and(|(file_id, _)| !kept(file_id))
        {
            self.warming_geom = None;
        }
        self.ready.retain(kept);
        self.targets = targets;
    }

    pub fn forget_all(&mut self) {
        *self = BackdropPreloads::default();
    }
}

/// Parse one candidate zone's MZB and model chunks into the same caches the
/// visible loader uses, so a flip finds everything resident. Runs inside a
/// per-frame slice; nothing here touches the world.
pub fn preload_backdrop_zones(
    mut preloads: ResMut<BackdropPreloads>,
    dat_root: Res<SharedDatRoot>,
    mut geom_cache: ResMut<ZoneGeomCache>,
    mut parse_cache: ResMut<MmbParseCache>,
    mut tex_pools: ResMut<MmbTexPools>,
    mut images: ResMut<Assets<Image>>,
    settings: Res<kuluu_render::GraphicsSettings>,
) {
    if preloads.targets.is_empty() && preloads.parsing.is_empty() && preloads.warming_geom.is_none()
    {
        return;
    }
    let slice = Instant::now();

    land_warmed_geometry(&mut preloads, &mut geom_cache, &parse_cache);

    let quality = TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    let parsed_tasks = std::mem::take(&mut preloads.parsing);
    let mut still_running = HashMap::new();
    for (asset, mut task) in parsed_tasks {
        if slice.elapsed().as_millis() >= PRELOAD_SLICE_MS {
            still_running.insert(asset, task);
            continue;
        }
        match future::block_on(future::poll_once(&mut task)) {
            None => {
                still_running.insert(asset, task);
            }
            Some(loaded) => cache_warmed_model(
                &asset,
                loaded,
                quality,
                &mut parse_cache,
                &mut tex_pools,
                &mut images,
            ),
        }
    }
    preloads.parsing = still_running;

    pump_model_parses(
        &mut preloads,
        &geom_cache,
        &parse_cache,
        dat_root.get().cloned(),
        slice,
    );
    mark_completed_zones(&mut preloads);
}

fn land_warmed_geometry(
    preloads: &mut BackdropPreloads,
    geom_cache: &mut ZoneGeomCache,
    parse_cache: &MmbParseCache,
) {
    let Some((file_id, task)) = preloads.warming_geom.as_mut() else {
        return;
    };
    let file = *file_id;
    let Some(geom) = future::block_on(future::poll_once(task)) else {
        return;
    };
    preloads.warming_geom = None;
    geom_cache.insert(zone_geom_key(file), geom);
    seed_model_queue(preloads, geom_cache, parse_cache, file);
}

/// Every model chunk a warmed zone places, nearest its own centre first. The
/// full set is what makes the zone complete, which is what a loading state now
/// waits for; nearest-first only decides what appears first while it drains.
/// Chunks already sitting in the parse cache (the visible loader may have
/// streamed this block before it became a warm target) are not queued again.
fn seed_model_queue(
    preloads: &mut BackdropPreloads,
    geom_cache: &ZoneGeomCache,
    parse_cache: &MmbParseCache,
    file_id: u32,
) {
    let Some(geom) = geom_cache
        .entries
        .iter()
        .find(|(key, _)| *key == zone_geom_key(file_id))
        .map(|(_, geom)| geom)
    else {
        preloads.ready.insert(file_id);
        return;
    };
    let Ok(build) = geom.mmb_spawns.as_ref() else {
        preloads.ready.insert(file_id);
        return;
    };
    let mut chunks: Vec<(usize, f32)> = Vec::new();
    let centre = spawn_centre(&build.spawns);
    for spawn in &build.spawns {
        let t = spawn.bevy_transform.col(3);
        let translation = Vec2::new(t.x, t.y);
        if parse_cache
            .by_asset
            .contains_key(&(file_id, spawn.chunk_idx))
        {
            continue;
        }
        let dist_sq = (translation - centre).length_squared();
        match chunks.iter_mut().find(|(idx, _)| *idx == spawn.chunk_idx) {
            Some(existing) => existing.1 = existing.1.min(dist_sq),
            None => chunks.push((spawn.chunk_idx, dist_sq)),
        }
    }
    chunks.sort_by(|a, b| a.1.total_cmp(&b.1));
    let queue: VecDeque<usize> = chunks.into_iter().map(|(chunk_idx, _)| chunk_idx).collect();
    if queue.is_empty() {
        preloads.ready.insert(file_id);
    } else {
        preloads.pending_models.insert(file_id, queue);
    }
}

fn spawn_centre(spawns: &[kuluu_render::dat_mzb::ZoneMmbSpawn]) -> Vec2 {
    let Some(first) = spawns.first() else {
        return Vec2::ZERO;
    };
    let xz = |m: Mat4| {
        let c = m.col(3);
        Vec2::new(c.x, c.y)
    };
    let mut min = xz(first.bevy_transform);
    let mut max = min;
    for spawn in spawns.iter().skip(1) {
        let p = xz(spawn.bevy_transform);
        min = min.min(p);
        max = max.max(p);
    }
    (min + max) * 0.5
}

fn pump_model_parses(
    preloads: &mut BackdropPreloads,
    geom_cache: &ZoneGeomCache,
    parse_cache: &MmbParseCache,
    root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
    slice: Instant,
) {
    let Some(root) = root else { return };
    let targets = preloads.targets.clone();
    for file_id in targets {
        if preloads.parsing.len() >= PRELOAD_MAX_PARSE_INFLIGHT
            || slice.elapsed().as_millis() >= PRELOAD_SLICE_MS
        {
            return;
        }
        if preloads.ready.contains(&file_id) || preloads.warming_geom.is_some() {
            continue;
        }
        if !preloads.pending_models.contains_key(&file_id) {
            // Geometry either came from the cache (the visible loader or an
            // earlier warm) or has never been asked for.
            if geom_cache.contains(zone_geom_key(file_id)) {
                seed_model_queue(preloads, geom_cache, parse_cache, file_id);
                continue;
            }
            if preloads.warming_geom.is_none() {
                start_geometry_warm(preloads, file_id, &root);
            }
            continue;
        }
        let Some(queue) = preloads.pending_models.get_mut(&file_id) else {
            continue;
        };
        while let Some(chunk_idx) = queue.pop_front() {
            if parse_cache.by_asset.contains_key(&(file_id, chunk_idx)) {
                continue;
            }
            let root_arc = root.clone();
            let task = AsyncComputeTaskPool::get()
                .spawn(async move { dat_mmb::load_mmb(&root_arc, file_id, chunk_idx).ok() });
            preloads.parsing.insert((file_id, chunk_idx), task);
            break;
        }
    }
}

fn start_geometry_warm(
    preloads: &mut BackdropPreloads,
    file_id: u32,
    root: &std::sync::Arc<ffxi_dat::DatRoot>,
) {
    let root = root.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let (submeshes, instances) =
            match kuluu_render::dat_mzb::load_mzb_placed(&root, file_id, None) {
                Ok(loaded) => loaded,
                Err(_) => {
                    return LoadedZoneGeom {
                        submeshes: std::sync::Arc::new(Vec::new()),
                        instances: std::sync::Arc::new(Vec::new()),
                        mmb_spawns: Err(format!("warm: MZB {file_id} unreadable")),
                    }
                }
            };
        let mmb_spawns = kuluu_render::dat_mzb::build_zone_mmb_spawns(&root, file_id, None, None);
        LoadedZoneGeom {
            submeshes: std::sync::Arc::new(submeshes),
            instances: std::sync::Arc::new(instances),
            mmb_spawns,
        }
    });
    preloads.warming_geom = Some((file_id, task));
}

fn cache_warmed_model(
    asset: &(u32, usize),
    loaded: Option<LoadedMmb>,
    quality: TextureQuality,
    parse_cache: &mut MmbParseCache,
    tex_pools: &mut MmbTexPools,
    images: &mut Assets<Image>,
) {
    let (file_id, chunk_idx) = *asset;
    let Some(mut loaded) = loaded else {
        // A read that failed once is not retried every frame for the rest of the
        // session; the visible loader reports it when the flip happens.
        parse_cache.by_asset.insert((file_id, chunk_idx), None);
        return;
    };
    let pooled = tex_pools.by_file.contains_key(&file_id);
    if !pooled && !loaded.textures.is_empty() {
        let texture_count = loaded.textures.len();
        let pool =
            kuluu_render::dat_mmb::pool_loaded_textures(&loaded, texture_count, quality, images);
        tex_pools.by_file.insert(file_id, pool);
    } else {
        loaded.textures = Vec::new();
    }
    parse_cache
        .by_asset
        .insert((file_id, chunk_idx), Some(loaded));
    if tex_pools.by_file.contains_key(&file_id) {
        kuluu_render::dat_mmb::drop_pooled_textures(parse_cache, file_id);
    }
}

fn mark_completed_zones(preloads: &mut BackdropPreloads) {
    let parsing = &preloads.parsing;
    let finished: Vec<u32> = preloads
        .pending_models
        .iter()
        .filter(|(file_id, queue)| {
            queue.is_empty()
                && !parsing
                    .keys()
                    .any(|(parsing_file, _)| parsing_file == *file_id)
        })
        .map(|(file_id, _)| *file_id)
        .collect();
    for file_id in finished {
        preloads.pending_models.remove(&file_id);
        preloads.ready.insert(file_id);
    }
}
