#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use ffxi_dat::mmb::{parse_models, MmbHeader};
use ffxi_dat::texture::{decode_texture, DecodedTexture};
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

use crate::ffxi_zone_material::FfxiZoneMaterial;
use crate::graphics_settings::GraphicsSettings;
use crate::look_resolver::dispatch_look_driven_models;
use crate::scene::TrackedEntities;
use crate::zone_texture::{decoded_texture_to_image, TextureQuality};

#[derive(Component)]
pub struct MmbOverlay;

#[derive(Resource, Default)]
pub struct MmbHandleCache {
    pub mesh: std::collections::HashMap<(u32, usize, usize), bevy::asset::Handle<Mesh>>,
    pub material:
        std::collections::HashMap<(u32, usize, usize), bevy::asset::Handle<FfxiZoneMaterial>>,
}

#[derive(Resource, Default)]
pub struct MmbLoadQueue {
    pub pending: std::collections::VecDeque<LoadMmbRequest>,
}

#[derive(Resource, Default)]
pub struct MmbParseCache {
    pub by_asset: std::collections::HashMap<(u32, usize), Option<LoadedMmb>>,
}

#[derive(Resource, Default)]
pub struct MmbLoadInFlight {
    pub tasks: std::collections::HashMap<(u32, usize), Task<Option<LoadedMmb>>>,
}

#[derive(Resource, Default)]
pub struct MmbTexPools {
    pub by_file: std::collections::HashMap<
        u32,
        (
            std::collections::HashMap<String, Handle<Image>>,
            Option<Handle<Image>>,
        ),
    >,
}

/// Last texture-filtering anisotropy applied to the pooled images, so the
/// live-apply system can skip redundant GPU re-uploads when an unrelated
/// graphics setting changes.
#[derive(Resource, Default)]
pub struct AppliedTextureFiltering {
    pub anisotropy: Option<u16>,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMmbRequest {
    pub file_id: u32,
    pub chunk_idx: usize,

    pub world_pos: Vec3,
    pub entity_id: Option<u32>,

    pub world_transform: Option<Mat4>,
}

pub struct DatOverlayPlugin;

impl Plugin for DatOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LoadMmbRequest>()
            .add_message::<crate::dat_vos2::LoadVos2Request>()
            .add_message::<crate::ffxi_actor_render::LoadActorRequest>()
            .add_message::<crate::dat_mzb::LoadMzbRequest>()
            .init_resource::<MmbHandleCache>()
            .init_resource::<MmbLoadQueue>()
            .init_resource::<MmbParseCache>()
            .init_resource::<MmbLoadInFlight>()
            .init_resource::<MmbTexPools>()
            .init_resource::<AppliedTextureFiltering>()
            .init_resource::<crate::dat_mzb::LastAutoLoadedZone>()
            .init_resource::<crate::dat_mzb::DrawDistance>()
            .init_resource::<crate::dat_mzb::MzbCollisionGeometry>()
            .init_resource::<crate::dat_mzb::LoadMzbInFlight>()
            .init_resource::<crate::dat_mzb::ZoneGeomCache>()
            .init_resource::<crate::dat_mzb::PendingWaterSpawns>()
            .init_resource::<crate::ffxi_actor_render::ActorLoadInFlight>()
            .add_systems(
                Update,
                (
                    crate::dat_mzb::auto_load_zone_geometry_system,
                    dispatch_look_driven_models,
                    crate::dat_mzb::kick_load_mzb_tasks,
                    crate::dat_mzb::poll_load_mzb_tasks,
                    crate::dat_mzb::spawn_zone_water,
                    process_load_mmb_requests,
                    crate::ffxi_actor_render::kick_load_actor_tasks,
                    crate::ffxi_actor_render::poll_load_actor_tasks,
                    crate::ffxi_actor_render::tick_morph_in,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    crate::dat_mzb::cull_entities_by_distance,
                    crate::dat_mzb::apply_zone_geom_visibility,
                ),
            )
            .add_systems(
                Update,
                apply_texture_filtering_system.run_if(resource_changed::<GraphicsSettings>),
            );
    }
}

pub struct MmbSubMesh {
    pub variant_name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,

    pub blending: u16,
}

#[derive(Debug, Clone)]
pub struct NamedTexture {
    pub name: String,
    pub texture: DecodedTexture,
}

pub struct LoadedMmb {
    pub submeshes: Vec<MmbSubMesh>,
    pub textures: Vec<NamedTexture>,

    pub asset_name: String,

    /// Header bytes 16..32 (XIM's section `name`). A leading '_' selects the
    /// alpha-tested cutout render mode for this model's submeshes.
    pub zone_mesh_name: String,
}

pub fn load_mmb(file_id: u32, chunk_idx: usize) -> Result<LoadedMmb, String> {
    let root =
        DatRoot::from_env_or_default().map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = chunks.get(chunk_idx).ok_or_else(|| {
        format!(
            "file has {} chunks, idx {chunk_idx} out of range",
            chunks.len()
        )
    })?;
    if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
        return Err(format!(
            "chunk {chunk_idx} kind={:#x} ({:?}), not an MMB",
            chunk.kind,
            ChunkKind::label(chunk.kind),
        ));
    }

    let decrypted = mmb::decrypt(chunk.data).map_err(|e| format!("decrypt: {e}"))?;
    let header = MmbHeader::parse(&decrypted).map_err(|e| format!("header parse: {e}"))?;

    let models = parse_models(&decrypted);

    let textures: Vec<NamedTexture> = chunks
        .iter()
        .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Img))
        .filter_map(|c| {
            let texture = decode_texture(c.data).ok()?;
            let name = ffxi_dat::texture::extract_texture_name(c.data).unwrap_or_default();
            Some(NamedTexture { name, texture })
        })
        .collect();

    let mut out = Vec::with_capacity(models.len());
    for m in &models {
        if m.vertices.is_empty() || m.indices.is_empty() {
            continue;
        }

        const COORD_SANE_LIMIT: f32 = 10_000.0;
        if m.vertices.iter().any(|v| {
            v.pos
                .iter()
                .any(|c| !c.is_finite() || c.abs() > COORD_SANE_LIMIT)
        }) {
            continue;
        }
        let positions: Vec<[f32; 3]> = m.vertices.iter().map(|v| v.pos).collect();
        let normals: Vec<[f32; 3]> = m.vertices.iter().map(|v| v.normal).collect();
        let uvs: Vec<[f32; 2]> = m.vertices.iter().map(|v| v.uv).collect();

        let colors: Vec<[f32; 4]> = m
            .vertices
            .iter()
            .map(|v| {
                [
                    v.rgba[0] as f32 / 128.0,
                    v.rgba[1] as f32 / 128.0,
                    v.rgba[2] as f32 / 128.0,
                    v.rgba[3] as f32 / 128.0,
                ]
            })
            .collect();

        let vert_count = m.vertices.len() as u16;
        let indices: Vec<u32> = m
            .indices
            .chunks_exact(3)
            .filter(|t| t[0] < vert_count && t[1] < vert_count && t[2] < vert_count)
            .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();
        if indices.is_empty() {
            continue;
        }
        out.push(MmbSubMesh {
            variant_name: m.texture_name.clone(),
            positions,
            normals,
            uvs,
            colors,
            indices,
            blending: m.blending,
        });
    }

    let asset_name = header.asset_name_str().trim().to_string();
    // XIM (`ZoneMeshSection.kt`): the model name at header bytes 16..32 is the
    // alpha-test selector — a leading '_' marks a cutout (foliage) model.
    let zone_mesh_name = header.zone_mesh_name();
    Ok(LoadedMmb {
        submeshes: out,
        textures,
        asset_name,
        zone_mesh_name,
    })
}

fn is_zone_placement(req: &LoadMmbRequest) -> bool {
    req.entity_id.is_none() && req.world_transform.is_some()
}

fn mmb_dist_sq_xz(req: &LoadMmbRequest, self_pos: Vec3) -> f32 {
    let p = req
        .world_transform
        .map(|m| m.w_axis.truncate())
        .unwrap_or(req.world_pos);
    let dx = p.x - self_pos.x;
    let dz = p.z - self_pos.z;
    dx * dx + dz * dz
}

fn mmb_load_order_key(req: &LoadMmbRequest, self_pos: Vec3) -> f32 {
    if is_zone_placement(req) {
        mmb_dist_sq_xz(req, self_pos)
    } else {
        -1.0
    }
}

pub fn process_load_mmb_requests(
    mut events: MessageReader<LoadMmbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    tracked: Res<TrackedEntities>,
    mut handle_cache: ResMut<MmbHandleCache>,
    mut queue: ResMut<MmbLoadQueue>,
    mut parse_cache: ResMut<MmbParseCache>,
    mut tex_pools_res: ResMut<MmbTexPools>,
    settings: Res<GraphicsSettings>,
    self_q: Query<&GlobalTransform, With<crate::components::IsSelf>>,
    mut in_flight: ResMut<MmbLoadInFlight>,
) {
    let mut newly_parsed: Vec<((u32, usize), Option<LoadedMmb>)> = Vec::new();
    in_flight.tasks.retain(
        |asset, task| match future::block_on(future::poll_once(task)) {
            Some(result) => {
                newly_parsed.push((*asset, result));
                false
            }
            None => true,
        },
    );
    for (asset, result) in newly_parsed {
        parse_cache.by_asset.entry(asset).or_insert(result);
    }

    queue.pending.extend(events.read().copied());
    if queue.pending.is_empty() {
        return;
    }

    let self_pos = self_q.single().ok().map(|t| t.translation());
    if let Some(self_pos) = self_pos {
        queue.pending.make_contiguous().sort_by(|a, b| {
            mmb_load_order_key(a, self_pos).total_cmp(&mmb_load_order_key(b, self_pos))
        });
    }
    let load_radius = settings.view_distance * crate::dat_mzb::MMB_LOAD_DISTANCE_MARGIN;
    let load_radius_sq = load_radius * load_radius;

    let mut mmb_logged: std::collections::HashSet<(u32, usize)> = std::collections::HashSet::new();

    let diag_file_id: Option<u32> = match std::env::var("FFXI_DIAG_ZONE_GEOM") {
        Ok(s) if s == "*" || s == "all" || s.eq_ignore_ascii_case("any") => Some(u32::MAX),
        Ok(s) => s.parse::<u32>().ok(),
        _ => None,
    };
    let mut diag_zero_submesh: std::collections::HashMap<u32, Vec<(usize, String)>> =
        std::collections::HashMap::new();
    let mut diag_loaded: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut diag_load_failed: std::collections::HashMap<u32, u32> =
        std::collections::HashMap::new();
    let diag_matches = |fid: u32| -> bool {
        match diag_file_id {
            Some(u32::MAX) => true,
            Some(want) => want == fid,
            None => false,
        }
    };

    const MMB_SPAWN_BUDGET: usize = 96;
    const HEAVY: usize = 8;
    const MMB_MAX_INFLIGHT: usize = 64;
    let mut spawned = 0usize;
    let mut retained: std::collections::VecDeque<LoadMmbRequest> =
        std::collections::VecDeque::with_capacity(queue.pending.len());

    while let Some(req) = queue.pending.pop_front() {
        if let Some(self_pos) = self_pos {
            if is_zone_placement(&req) && mmb_dist_sq_xz(&req, self_pos) > load_radius_sq {
                retained.push_back(req);
                retained.append(&mut queue.pending);
                break;
            }
        }

        let asset = (req.file_id, req.chunk_idx);
        match parse_cache.by_asset.get(&asset) {
            Some(Some(loaded)) => {
                if diag_matches(req.file_id) {
                    *diag_loaded.entry(req.file_id).or_insert(0) += 1;
                }

                if loaded.submeshes.is_empty() {
                    if diag_matches(req.file_id) {
                        diag_zero_submesh
                            .entry(req.file_id)
                            .or_default()
                            .push((req.chunk_idx, loaded.asset_name.clone()));
                    }

                    if req.world_transform.is_none() {
                        push_system_msg(
                            &mut toasts,
                            format!(
                                "/load_mmb {} {}: 0 renderable sub-records",
                                req.file_id, req.chunk_idx,
                            ),
                        );
                    }
                    continue;
                }

                let pool_exists = tex_pools_res.by_file.contains_key(&req.file_id);
                let cost = if pool_exists { 1 } else { HEAVY };
                if spawned > 0 && spawned + cost > MMB_SPAWN_BUDGET {
                    retained.push_back(req);
                    continue;
                }
                spawned += cost;

                let texture_count = loaded.textures.len();
                let quality = TextureQuality {
                    mipmaps: settings.texture_filtering.mipmaps(),
                    anisotropy: settings.texture_filtering.anisotropy(),
                };
                let pool = tex_pools_res.by_file.entry(req.file_id).or_insert_with(|| {
                    let mut by_name: std::collections::HashMap<String, Handle<Image>> =
                        std::collections::HashMap::with_capacity(texture_count);
                    let mut first: Option<Handle<Image>> = None;
                    for nt in &loaded.textures {
                        let handle = images.add(decoded_texture_to_image(&nt.texture, quality));
                        if first.is_none() {
                            first = Some(handle.clone());
                        }
                        if !nt.name.is_empty() {
                            by_name.insert(nt.name.clone(), handle);
                        }
                    }
                    (by_name, first)
                });
                let tex_by_name = &pool.0;
                let first_texture = pool.1.clone();

                if mmb_logged.insert((req.file_id, req.chunk_idx)) {
                    let mut img_stats: Vec<(String, u8, u8)> = loaded
                        .textures
                        .iter()
                        .filter(|nt| !nt.name.is_empty())
                        .map(|nt| {
                            let (mut amin, mut amax) = (255u8, 0u8);
                            for px in nt.texture.rgba.chunks_exact(4) {
                                amin = amin.min(px[3]);
                                amax = amax.max(px[3]);
                            }
                            (nt.name.clone(), amin, amax)
                        })
                        .collect();
                    img_stats.sort_by(|a, b| a.0.cmp(&b.0));
                    let img_names: Vec<String> = img_stats
                        .into_iter()
                        .map(|(n, amin, amax)| format!("{n} α[{amin}..{amax}]"))
                        .collect();
                    let mut requested: Vec<&str> = loaded
                        .submeshes
                        .iter()
                        .map(|s| s.variant_name.as_str())
                        .collect();
                    requested.sort_unstable();
                    requested.dedup();
                    let (matched, unmatched): (Vec<&str>, Vec<&str>) = requested
                        .iter()
                        .partition(|n| tex_by_name.contains_key(**n));

                    let mut blending_view: Vec<(String, u16)> = loaded
                        .submeshes
                        .iter()
                        .map(|s| (s.variant_name.clone(), s.blending))
                        .collect();
                    blending_view.sort_by(|a, b| a.0.cmp(&b.0));
                    let blending_strs: Vec<String> = blending_view
                        .into_iter()
                        .map(|(name, b)| format!("{name}:0x{b:04X}"))
                        .collect();
                    debug!(
                        target: "ffxi_viewer_core::dat_mmb",
                        file_id = req.file_id,
                        chunk_idx = req.chunk_idx,
                        asset = %loaded.asset_name,
                        mesh_name = %loaded.zone_mesh_name,
                        cutout = loaded.zone_mesh_name.starts_with('_'),
                        submesh_count = loaded.submeshes.len(),
                        img_count = tex_by_name.len(),
                        imgs = ?img_names,
                        matched = ?matched,
                        unmatched = ?unmatched,
                        blending = ?blending_strs,
                        first_fallback = first_texture.is_some(),
                        "MMB texture pool",
                    );
                }

                let is_static_placement = req
                    .entity_id
                    .and_then(|id| tracked.by_id.get(&id))
                    .is_none();
                let parent = match req.entity_id.and_then(|id| tracked.by_id.get(&id)) {
                    Some(&bevy_e) => {
                        commands.entity(bevy_e).remove::<Mesh3d>();
                        bevy_e
                    }
                    None => {
                        if let Some(missing) = req.entity_id {
                            push_system_msg(
                                &mut toasts,
                                format!(
                            "/load_mmb_on {missing} {} {}: no tracked entity for id {missing} \
                             — spawning at world_pos instead",
                            req.file_id, req.chunk_idx,
                        ),
                            );
                        }
                        let parent_transform = match req.world_transform {
                            Some(m) => Transform::from_matrix(m),
                            None => Transform::from_translation(req.world_pos),
                        };
                        let is_zone_spawn =
                            req.entity_id.is_none() && req.world_transform.is_some();

                        let mut e = commands.spawn((
                            MmbOverlay,
                            crate::components::InGameEntity,
                            parent_transform,
                            Visibility::default(),
                        ));
                        if is_zone_spawn {
                            e.insert(crate::dat_mzb::AutoMzbOverlay);
                        }
                        e.id()
                    }
                };

                let n_subs = loaded.submeshes.len();
                for (sub_index, sub) in loaded.submeshes.iter().enumerate() {
                    let cache_key = (req.file_id, req.chunk_idx, sub_index);

                    let mesh_handle = handle_cache
                        .mesh
                        .entry(cache_key)
                        .or_insert_with(|| {
                            let mut mesh = Mesh::new(
                                PrimitiveTopology::TriangleList,
                                RenderAssetUsages::default(),
                            );
                            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, sub.positions.clone());
                            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, sub.normals.clone());
                            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
                            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, sub.colors.clone());
                            mesh.insert_indices(Indices::U32(sub.indices.clone()));
                            meshes.add(mesh)
                        })
                        .clone();

                    let variant_trimmed = sub.variant_name.trim();
                    let sub_texture = tex_by_name
                        .get(variant_trimmed)
                        .cloned()
                        .or_else(|| first_texture.clone());

                    let (alpha_mode, discard_threshold) = submesh_alpha_mode(
                        &loaded.zone_mesh_name,
                        sub.blending,
                        sub_texture.is_some(),
                    );
                    let has_texture = if sub_texture.is_some() { 1.0 } else { 0.0 };
                    let blend_flag = if matches!(alpha_mode, AlphaMode::Blend) {
                        1.0
                    } else {
                        0.0
                    };

                    let mat_handle = handle_cache
                        .material
                        .entry(cache_key)
                        .or_insert_with(|| {
                            materials.add(FfxiZoneMaterial::new(
                                sub_texture,
                                crate::skinned_ffxi_material::FfxiMaterialFlags {
                                    flags: Vec4::new(
                                        has_texture,
                                        blend_flag,
                                        0.0,
                                        discard_threshold,
                                    ),
                                },
                                Vec4::ONE,
                                Vec4::ZERO,
                                alpha_mode,
                            ))
                        })
                        .clone();

                    let mut child = commands.spawn((
                        MmbOverlay,
                        Mesh3d(mesh_handle),
                        MeshMaterial3d(mat_handle),
                        Transform::default(),
                        ChildOf(parent),
                    ));

                    if is_static_placement {
                        child.insert(crate::components::CameraOccluder);
                    }

                    child.insert(crate::hud::mesh_debug::mesh_debug_bundle(
                        crate::hud::mesh_debug::MmbDebugInfo {
                            file_id: req.file_id,
                            chunk_idx: req.chunk_idx,
                            sub_index,
                            asset_name: loaded.asset_name.clone(),
                            variant_name: sub.variant_name.trim().to_string(),
                        },
                    ));
                }

                let is_zone_spawn = req.entity_id.is_none() && req.world_transform.is_some();
                if !is_zone_spawn {
                    let where_ = match req.entity_id {
                        Some(id) => format!("on entity {id}"),
                        None => format!(
                            "at ({:.1}, {:.1}, {:.1})",
                            req.world_pos.x, req.world_pos.y, req.world_pos.z,
                        ),
                    };
                    let tex_note = match texture_count {
                        0 => " (no texture)".to_string(),
                        1 => " +1 texture".to_string(),
                        n => format!(" +{n} textures"),
                    };
                    push_system_msg(
                        &mut toasts,
                        format!(
                            "/load_mmb {} {}: spawned {n_subs} sub-mesh{} {where_}{tex_note}",
                            req.file_id,
                            req.chunk_idx,
                            if n_subs == 1 { "" } else { "es" },
                        ),
                    );
                }
            }
            Some(None) => {
                push_system_msg(
                    &mut toasts,
                    format!("/load_mmb {} {}: load failed", req.file_id, req.chunk_idx),
                );
                if diag_matches(req.file_id) {
                    *diag_load_failed.entry(req.file_id).or_insert(0) += 1;
                }
            }
            None => {
                if !in_flight.tasks.contains_key(&asset) && in_flight.tasks.len() < MMB_MAX_INFLIGHT
                {
                    let pool = AsyncComputeTaskPool::get();
                    let (file_id, chunk_idx) = (req.file_id, req.chunk_idx);
                    in_flight.tasks.insert(
                        asset,
                        pool.spawn(async move { load_mmb(file_id, chunk_idx).ok() }),
                    );
                }
                retained.push_back(req);
            }
        }
    }
    queue.pending = retained;

    if diag_file_id.is_some() {
        for (fid, examples) in &diag_zero_submesh {
            if examples.is_empty() {
                continue;
            }
            let loaded = diag_loaded.get(fid).copied().unwrap_or(0);
            let load_failed = diag_load_failed.get(fid).copied().unwrap_or(0);
            let head: Vec<&(usize, String)> = examples.iter().take(20).collect();
            info!(
                target: "ffxi_viewer_core::dat_mmb::diag",
                file_id = *fid,
                loaded,
                load_failed,
                zero_submesh = examples.len(),
                "DIAG-zonegeom zero-submesh MMBs (chunk_idx, asset_name, top 20): {head:?}",
            );
        }

        for (fid, loaded) in &diag_loaded {
            if diag_zero_submesh
                .get(fid)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
            {
                continue;
            }
            let load_failed = diag_load_failed.get(fid).copied().unwrap_or(0);
            info!(
                target: "ffxi_viewer_core::dat_mmb::diag",
                file_id = *fid,
                loaded = *loaded,
                load_failed,
                zero_submesh = 0,
                "DIAG-zonegeom MMB pass: all submeshes non-empty",
            );
        }
    }
}

/// Choose an MMB submesh's render mode, per XIM (`research/xim` ·
/// `ZoneMeshSection.kt`). The model name (header bytes 16..32, our
/// `zone_mesh_name`) starting with '_' selects an alpha-tested cutout at
/// XIM's `discardThreshold` of 0.375; the `0x8000` flag bit marks translucency
/// (water/glass), rendered as `AlphaMode::Blend` — the zone shader emits real
/// alpha for these (see `flags.y` in `zone_ffxi.wgsl`). Everything else is
/// opaque. The render mode is NEVER derived from texture alpha content — doing
/// so punches holes in ordinary opaque ground/wall textures that carry
/// incidental transparency.
fn submesh_alpha_mode(zone_mesh_name: &str, blending: u16, has_texture: bool) -> (AlphaMode, f32) {
    if !has_texture {
        (AlphaMode::Opaque, 0.0)
    } else if zone_mesh_name.starts_with('_') {
        (AlphaMode::Mask(0.375), 0.375)
    } else if (blending & 0x8000) != 0 {
        (AlphaMode::Blend, 0.0)
    } else {
        (AlphaMode::Opaque, 0.0)
    }
}

fn push_system_msg(toasts: &mut MessageWriter<crate::snapshot::ToastEvent>, text: String) {
    toasts.write(crate::snapshot::ToastEvent::debug(text));
}

/// Re-sample the already-pooled MMB textures when the Texture Filtering setting
/// changes. Only the sampler's anisotropy varies live; the bilinear+mip data is
/// baked at load, so we patch the sampler in place rather than rebuild images.
/// The applied-value guard skips the GPU re-upload when an unrelated graphics
/// setting triggered the change.
pub fn apply_texture_filtering_system(
    settings: Res<GraphicsSettings>,
    pools: Res<MmbTexPools>,
    mut images: ResMut<Assets<Image>>,
    mut applied: ResMut<AppliedTextureFiltering>,
) {
    let aniso = settings.texture_filtering.anisotropy();
    if applied.anisotropy == Some(aniso) {
        return;
    }
    let mut patch = |handle: &Handle<Image>| {
        if let Some(img) = images.get_mut(handle) {
            img.sampler = bevy::image::ImageSampler::Descriptor(
                crate::zone_texture::sampler_descriptor(aniso),
            );
        }
    };
    for (by_name, first) in pools.by_file.values() {
        for handle in by_name.values() {
            patch(handle);
        }
        if let Some(handle) = first {
            patch(handle);
        }
    }
    applied.anisotropy = Some(aniso);
}

#[cfg(test)]
mod tests {
    use super::{mmb_dist_sq_xz, mmb_load_order_key, submesh_alpha_mode, LoadMmbRequest};
    use crate::zone_texture::ffxi_alpha_remap;
    use bevy::prelude::{AlphaMode, Mat4, Vec3};

    fn zone_placement_at(pos: Vec3) -> LoadMmbRequest {
        LoadMmbRequest {
            file_id: 0,
            chunk_idx: 0,
            world_pos: Vec3::ZERO,
            entity_id: None,
            world_transform: Some(Mat4::from_translation(pos)),
        }
    }

    fn entity_spawn_at(pos: Vec3) -> LoadMmbRequest {
        LoadMmbRequest {
            file_id: 0,
            chunk_idx: 0,
            world_pos: pos,
            entity_id: Some(7),
            world_transform: None,
        }
    }

    #[test]
    fn dist_key_ignores_vertical_axis() {
        let self_pos = Vec3::new(10.0, 999.0, 20.0);
        let req = zone_placement_at(Vec3::new(13.0, -50.0, 24.0));
        assert_eq!(mmb_dist_sq_xz(&req, self_pos), 3.0 * 3.0 + 4.0 * 4.0);
    }

    #[test]
    fn entity_spawns_sort_ahead_of_any_zone_placement() {
        let self_pos = Vec3::ZERO;
        let entity = mmb_load_order_key(&entity_spawn_at(Vec3::new(500.0, 0.0, 500.0)), self_pos);
        let nearest_prop =
            mmb_load_order_key(&zone_placement_at(Vec3::new(0.1, 0.0, 0.0)), self_pos);
        assert!(entity < nearest_prop);
    }

    #[test]
    fn nearer_zone_placement_sorts_first() {
        let self_pos = Vec3::ZERO;
        let near = mmb_load_order_key(&zone_placement_at(Vec3::new(5.0, 0.0, 0.0)), self_pos);
        let far = mmb_load_order_key(&zone_placement_at(Vec3::new(50.0, 0.0, 0.0)), self_pos);
        assert!(near < far);
    }

    #[test]
    fn underscore_model_is_cutout_at_xim_threshold() {
        // XIM: name.startsWith("_") -> discardThreshold 0.375f.
        let (mode, t) = submesh_alpha_mode("_yashi", 0x0000, true);
        assert_eq!(mode, AlphaMode::Mask(0.375));
        assert_eq!(t, 0.375);
    }

    #[test]
    fn blend_flag_is_translucent_non_underscore_model() {
        // XIM `ZoneMeshSection` 0x8000 -> real alpha blend (water/glass), not a
        // cutout. discard threshold is 0.0 so the shader's mask test never fires.
        let (mode, t) = submesh_alpha_mode("kabuse_m", 0x8000, true);
        assert_eq!(mode, AlphaMode::Blend);
        assert_eq!(t, 0.0);
    }

    #[test]
    fn plain_ground_model_stays_opaque() {
        // The regression: a non-underscore model with a non-blend flag (e.g.
        // back-face-cull-disable 0x2000) and incidental texture alpha must NOT
        // be alpha-tested, or the ground gets punched into holes.
        let (mode, t) = submesh_alpha_mode("ground01", 0x2000, true);
        assert_eq!(mode, AlphaMode::Opaque);
        assert_eq!(t, 0.0);
        let (mode, _) = submesh_alpha_mode("ground01", 0x0000, true);
        assert_eq!(mode, AlphaMode::Opaque);
    }

    #[test]
    fn textureless_submesh_is_opaque_even_if_underscore() {
        let (mode, _) = submesh_alpha_mode("_yashi", 0x8000, false);
        assert_eq!(mode, AlphaMode::Opaque);
    }

    #[test]
    fn ffxi_alpha_remap_obeys_lotus_spec() {
        assert_eq!(ffxi_alpha_remap(0), 0);
        assert_eq!(ffxi_alpha_remap(15), 0);

        assert_eq!(ffxi_alpha_remap(128), 255);
        assert_eq!(ffxi_alpha_remap(136), 255);
        assert_eq!(ffxi_alpha_remap(255), 255);

        let mut prev = 0u8;
        for raw in 0u16..=255 {
            let cur = ffxi_alpha_remap(raw as u8);
            assert!(
                cur >= prev,
                "remap not monotonic at raw={raw}: prev={prev}, cur={cur}"
            );
            prev = cur;
        }

        for raw in 128u16..=255 {
            assert_eq!(
                ffxi_alpha_remap(raw as u8),
                255,
                "raw {raw} should saturate to 255"
            );
        }
    }
}
