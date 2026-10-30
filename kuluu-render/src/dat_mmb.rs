#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use ffxi_dat::mmb::{parse_models, MmbHeader};
use ffxi_dat::texture::{decode_texture, DecodedTexture, QualifiedTextureName};
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

use crate::ffxi_zone_material::{FfxiZoneMaterial, ZoneMaterialTouched};
use crate::graphics_settings::GraphicsSettings;
use crate::look_resolver::dispatch_look_driven_models;
use crate::scene::TrackedEntities;
use crate::zone_texture::{decoded_texture_to_image, TextureQuality};

#[derive(Component)]
pub struct MmbOverlay;

#[derive(Resource, Default)]
pub struct MmbHandleCache {
    pub mesh: std::collections::HashMap<(u32, usize, usize), bevy::asset::Handle<Mesh>>,
    pub material: std::collections::HashMap<
        (
            u32,
            usize,
            usize,
            bool,
            crate::ffxi_zone_material::ZoneLightBindings,
            u32,
        ),
        bevy::asset::Handle<FfxiZoneMaterial>,
    >,
}

impl MmbLoadQueue {
    /// Whether a queued zone placement still sits inside `radius_sq` of `from`.
    /// Placements beyond the load radius stay queued until the view approaches
    /// them, so this false means everything the current view would show has been
    /// spawned.
    pub fn pending_within(&self, from: Vec3, radius_sq: f32) -> bool {
        self.pending
            .iter()
            .any(|req| is_zone_placement(req) && mmb_dist_sq_xz(req, from) <= radius_sq)
    }
}

/// Loads every queued zone placement whatever its distance from the view. The
/// launcher screens run their stream with this on so "loaded" means the whole
/// block: the veil lifts once, onto a complete background, and never mid-stream.
#[derive(Resource, Default)]
pub struct StreamEverything(pub bool);

/// How far and how wide the stream reaches. Grouped because the streaming system
/// already carries a full complement of parameters, and callers name this one in
/// schedule ordering.
#[derive(bevy::ecs::system::SystemParam)]
pub struct MmbStreamGate<'w> {
    pub draw: Res<'w, crate::dat_mzb::DrawDistance>,
    pub everything: Res<'w, StreamEverything>,
}

impl MmbLoadQueue {
    /// Whether any zone placement is still waiting to spawn. The launcher's
    /// loading states watch this rather than a distance-bounded subset.
    pub fn has_pending_zone_placements(&self) -> bool {
        self.pending.iter().any(is_zone_placement)
    }
}

#[derive(Resource, Default)]
pub struct MmbLoadQueue {
    pub pending: std::collections::VecDeque<LoadMmbRequest>,

    pub last_eval_pos: Option<Vec3>,

    pub budget_deferred: bool,
}

// Below one yalm of movement the distance-sorted streaming order cannot
// meaningfully change; re-sorting the retained far queue every frame anyway
// measured 0.47ms steady-state in the 100-PC Jeuno capture (2026-07-31).
const MMB_REEVAL_MOVE_YALMS: f32 = 1.0;

fn mmb_repass_needed(
    new_events: bool,
    parse_completed: bool,
    budget_deferred: bool,
    draw_distance_changed: bool,
    last_eval_pos: Option<Vec3>,
    self_pos: Option<Vec3>,
) -> bool {
    if new_events || parse_completed || budget_deferred || draw_distance_changed {
        return true;
    }
    match (last_eval_pos, self_pos) {
        (Some(prev), Some(now)) => {
            prev.distance_squared(now) > MMB_REEVAL_MOVE_YALMS * MMB_REEVAL_MOVE_YALMS
        }
        (None, None) => false,
        _ => true,
    }
}

#[derive(Resource, Default)]
pub struct MmbParseCache {
    pub by_asset: std::collections::HashMap<(u32, usize), Option<LoadedMmb>>,
}

/// Upload one file's decoded textures and return the pool entry its placements
/// resolve through. Split out so a zone can be warmed before it is ever drawn.
pub fn pool_loaded_textures(
    loaded: &LoadedMmb,
    texture_count: usize,
    quality: TextureQuality,
    images: &mut Assets<Image>,
) -> (
    std::collections::HashMap<String, Handle<Image>>,
    Option<Handle<Image>>,
    std::collections::HashMap<QualifiedTextureName, Handle<Image>>,
) {
    let mut by_name: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(texture_count);
    let mut first: Option<Handle<Image>> = None;
    let mut by_raw_name: std::collections::HashMap<QualifiedTextureName, Handle<Image>> =
        std::collections::HashMap::with_capacity(texture_count);
    for nt in &loaded.textures {
        let mut image = decoded_texture_to_image(&nt.texture, quality);
        image.asset_usage = RenderAssetUsages::RENDER_WORLD;
        let handle = images.add(image);
        if first.is_none() {
            first = Some(handle.clone());
        }
        if let Some(raw_name) = nt.raw_name {
            by_raw_name
                .entry(raw_name)
                .or_insert_with(|| handle.clone());
        }
        if !nt.name.is_empty() {
            by_name.insert(nt.name.clone(), handle);
        }
    }
    (by_name, first, by_raw_name)
}

/// Release the decoded texels every cached chunk of `file_id` holds, once that
/// file's `MmbTexPools` entry owns the uploaded handles. A file's Img chunks are
/// collected into *every* chunk's `LoadedMmb`, so the cache otherwise keeps one
/// full copy of the file's texture set per cached chunk index.
pub fn drop_pooled_textures(cache: &mut MmbParseCache, file_id: u32) {
    for ((fid, _), entry) in cache.by_asset.iter_mut() {
        if *fid != file_id {
            continue;
        }
        if let Some(loaded) = entry {
            loaded.textures = Vec::new();
        }
    }
}

#[derive(Resource, Default)]
pub struct MmbLoadInFlight {
    pub tasks: std::collections::HashMap<(u32, usize), Task<Option<LoadedMmb>>>,
}

/// Per file: textures by local name, the file's first texture, and textures by their 16 name bytes
/// (the key a static model resolves its parts by).
#[derive(Resource, Default)]
pub struct MmbTexPools {
    pub by_file: std::collections::HashMap<
        u32,
        (
            std::collections::HashMap<String, Handle<Image>>,
            Option<Handle<Image>>,
            std::collections::HashMap<QualifiedTextureName, Handle<Image>>,
        ),
    >,
}

/// A static model (one MMB per placed piece of furniture) hangs its submeshes off `frame` and draws them by
/// the static-model rules instead of the zone placement ones
/// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md A2).
#[derive(Debug, Clone, Copy)]
pub struct StaticModelTarget {
    pub frame: Entity,
    pub alpha_test: bool,
}

/// One submesh of a static model, with its own material so the piece can be tinted alone.
#[derive(Component, Debug, Clone)]
pub struct StaticModelPart {
    pub file_id: u32,
    pub texture_name: QualifiedTextureName,
    pub material: Handle<FfxiZoneMaterial>,
    pub textured: bool,
}

/// Last texture-filtering anisotropy applied to the pooled images, so the
/// live-apply system can skip redundant GPU re-uploads when an unrelated
/// graphics setting changes.
#[derive(Resource, Default)]
pub struct AppliedTextureFiltering {
    pub anisotropy: Option<u16>,
}

// A generator-driven water sheet (ffxi-dat Generator::parse_model_spawn): the
// broad canal/harbor water (e.g. Port Windurst tshimonosea1/2) is instanced by a
// zone Generator, not the MZB object list, and carries its own translucent tint
// (alpha < 1) and per-layer UV-scroll velocity. When a LoadMmbRequest carries
// this, the spawned submeshes render blended with the tint and scroll their UVs.
#[derive(Debug, Clone, Copy)]
pub struct GenWater {
    pub tint: Vec4,
    pub uv_scroll: Vec2,
    /// World-space AABB of the transformed sheet (MMB header bounds × the
    /// generator transform), used to suppress the placeholder MZB water plane
    /// where the retail sheet already covers the surface.
    pub world_min: Vec3,
    pub world_max: Vec3,
    /// The sheet is a generator element like any other, so it ranks in the transparent
    /// sort by retail's element key (element_sort.rs), not by its mesh alone.
    pub sort_bias: f32,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMmbRequest {
    pub area_id: u32,
    pub light_bindings: crate::ffxi_zone_material::ZoneLightBindings,
    pub voyage_backdrop: bool,
    pub file_id: u32,
    pub chunk_idx: usize,

    pub world_pos: Vec3,
    pub entity_id: Option<u32>,

    pub world_transform: Option<Mat4>,

    pub water: Option<GenWater>,

    pub lod: Option<crate::dat_mzb::ZoneMeshLod>,

    pub door: Option<crate::zone_doors::ZoneDoorLeaf>,

    /// Zone block this placement belongs to, stamped on the spawned entity as
    /// [`crate::dat_mzb::ZoneBlockSlot`] so retiring an interior finds it.
    /// Ignored for entity-attached models, which no zone block owns.
    pub slot: u8,

    /// `MmbPlacement::sub_area_link`; 0 for everything that is not the exterior
    /// shell of an interior.
    pub sub_area_link: u32,

    pub static_model: Option<StaticModelTarget>,
}

// Animates one generator water sheet's UV scroll. Each sheet owns its material
// (never the shared cache), so the two stacked Port Windurst layers scroll at
// their own velocities. uv_offset flows to the GPU through FfxiZoneMaterial's
// persistent instance buffer; get_mut_untracked avoids rebuilding the bind group
// every frame (same pattern as scroll_water_uv / zone_clouds).
#[derive(Component)]
pub struct GenWaterScroll {
    pub material: Handle<FfxiZoneMaterial>,
    pub uv_scroll: Vec2,
}

pub fn scroll_gen_water_uv(
    time: Res<Time>,
    q: Query<&GenWaterScroll>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut touched: ResMut<ZoneMaterialTouched>,
) {
    // The generator's 0x27/0x28 velocity is UV per retail frame, not per second
    // (research/xim TextureCoordinateUpdater; same convention as zone_clouds
    // drift_clouds) — raw seconds scrolls the sea 30× too slow to perceive.
    let t = time.elapsed_secs() * crate::scheduler_runtime::RETAIL_FPS;
    for gw in q.iter() {
        if let Some(mat) = materials.get_mut_untracked(&gw.material) {
            // fract keeps the offset small for f32 precision over long sessions;
            // the texture repeats so a whole-tile jump is invisible.
            let u = (gw.uv_scroll.x * t).fract();
            let v = (gw.uv_scroll.y * t).fract();
            mat.uv_offset = Vec4::new(u, v, 0.0, 0.0);
            touched.mark(gw.material.id());
        }
    }
}

/// Keeps static placements out of the sun's shadow pass unless
/// `GraphicsSettings::zone_shadow_cast` opts them in. Runs on settings edits
/// only; the spawn path stamps new placements itself.
pub fn apply_zone_shadow_cast(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    q: Query<
        (Entity, Has<bevy::light::NotShadowCaster>),
        (
            With<MmbOverlay>,
            Or<(
                With<crate::components::CameraOccluder>,
                With<StaticModelPart>,
            )>,
        ),
    >,
) {
    if !settings.is_changed() {
        return;
    }
    for (e, blocked) in &q {
        if settings.zone_shadow_cast == blocked {
            let mut ec = commands.entity(e);
            if blocked {
                ec.remove::<bevy::light::NotShadowCaster>();
            } else {
                ec.insert(bevy::light::NotShadowCaster);
            }
        }
    }
}

/// `drive_sub_area_activation` suppresses the doorway shell's collision and only
/// *writes* the replacement `LoadMzbRequest`; nothing is in flight until
/// `kick_load_mzb_tasks` reads it. Split across schedule runs, that leaves a
/// frame with the floor gone and `LoadMzbInFlight::any_pending()` false — the
/// gate `kuluu::view_native::input::ground_recovery_candidate` reads before
/// recovering the player upward (kuluu-oubj). Registered as one ordered unit and
/// driven whole by `sub_area_activation::doorway_tests`.
pub fn zone_load_dispatch_systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        crate::sub_area_activation::drive_sub_area_activation,
        dispatch_look_driven_models,
        crate::dat_mzb::kick_load_mzb_tasks,
    )
        .chain()
}

pub struct DatOverlayPlugin;

impl Plugin for DatOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::gpu_assets::GpuAssetResidencyPlugin);
        app.add_observer(crate::ffxi_actor_render::finish_actor_reveal);
        app.add_message::<LoadMmbRequest>()
            .add_message::<crate::dat_vos2::LoadVos2Request>()
            .add_message::<crate::ffxi_actor_render::LoadActorRequest>()
            .add_message::<crate::dat_mzb::LoadMzbRequest>()
            .add_message::<crate::sub_area_activation::SubAreaChanged>()
            .add_message::<crate::sub_area_activation::SetSubArea>()
            .init_resource::<crate::sub_area_activation::SubAreaActivation>()
            .init_resource::<MmbHandleCache>()
            .init_resource::<MmbLoadQueue>()
            .init_resource::<StreamEverything>()
            .init_resource::<MmbParseCache>()
            .init_resource::<MmbLoadInFlight>()
            .init_resource::<MmbTexPools>()
            .init_resource::<AppliedTextureFiltering>()
            .init_resource::<crate::dat_mzb::LastAutoLoadedZone>()
            .init_resource::<crate::dat_mzb::DrawDistance>()
            .init_resource::<crate::dat_mzb::MzbCollisionGeometry>()
            .init_resource::<crate::dat_mzb::ZoneAreaMap>()
            .init_resource::<crate::dat_mzb::ZoneChunkLightMap>()
            .init_resource::<crate::dat_mzb::LoadMzbInFlight>()
            .init_resource::<crate::dat_mzb::ZoneGeomCache>()
            .init_resource::<crate::dat_mzb::PendingWaterSpawns>()
            .init_resource::<crate::dat_mzb::ZoneWaterMaterial>()
            .init_resource::<crate::ffxi_actor_render::ActorLoadInFlight>()
            .init_resource::<crate::ffxi_actor_render::ActorDatRoot>()
            .add_systems(
                Update,
                (
                    crate::dat_mzb::auto_load_zone_geometry_system,
                    zone_load_dispatch_systems(),
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
                    crate::dat_mzb::resolve_draw_distance,
                    crate::dat_mzb::cull_entities_by_distance,
                    crate::dat_mzb::select_zone_mmb_lod,
                    crate::dat_mzb::apply_sub_area_shell_visibility,
                    crate::dat_mzb::apply_zone_geom_visibility,
                    crate::dat_mzb::scroll_water_uv,
                    scroll_gen_water_uv,
                ),
            )
            .add_systems(
                Update,
                apply_texture_filtering_system.run_if(resource_changed::<GraphicsSettings>),
            )
            .add_systems(
                Update,
                resolve_static_model_textures
                    .after(process_load_mmb_requests)
                    .run_if(resource_changed::<MmbTexPools>),
            );
    }
}

pub struct MmbSubMesh {
    pub variant_name: String,
    pub texture_name_raw: QualifiedTextureName,
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
    pub raw_name: Option<QualifiedTextureName>,
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

pub fn load_mmb(root: &DatRoot, file_id: u32, chunk_idx: usize) -> Result<LoadedMmb, String> {
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root);
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
            let raw_name = ffxi_dat::texture::texture_name_raw(c.data);
            Some(NamedTexture {
                name,
                raw_name,
                texture,
            })
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
            .map(|v| mmb::vertex_color_to_linear(v.rgba))
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
            texture_name_raw: m.texture_name_raw,
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
    if req.voyage_backdrop {
        return 0.0;
    }
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

/// A full water sheet is not a coplanar decal: decal z-bias would pull it toward
/// the camera and let it float over terrain that should occlude it, so the water
/// material keeps z_bias_level 0. A sea sheet hangs off a water generator, so it
/// takes the two-stage CMoD3m chain with the water tint as its TEXTUREFACTOR.
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
    gate: MmbStreamGate<'_>,
    anchor: crate::dat_mzb::ZoneViewAnchor<'_, '_>,
    mut in_flight: ResMut<MmbLoadInFlight>,
    actor_root: Res<crate::ffxi_actor_render::ActorDatRoot>,
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
    let parse_completed = !newly_parsed.is_empty();
    for (asset, mut result) in newly_parsed {
        if tex_pools_res.by_file.contains_key(&asset.0) {
            if let Some(loaded) = result.as_mut() {
                loaded.textures = Vec::new();
            }
        }
        parse_cache.by_asset.entry(asset).or_insert(result);
    }

    let pending_before = queue.pending.len();
    queue.pending.extend(events.read().copied());
    let new_events = queue.pending.len() != pending_before;
    if queue.pending.is_empty() {
        return;
    }

    let (self_pos, follows_character) = match anchor.stream() {
        Some((pos, character)) => (Some(pos), character),
        None => (None, false),
    };
    // An attract-mode view has nothing waiting behind it: its stream is a
    // background job, and the screens in front of it keep their frame rate.
    let background_stream = self_pos.is_some() && !follows_character;
    if !mmb_repass_needed(
        new_events,
        parse_completed,
        queue.budget_deferred,
        gate.draw.is_changed(),
        queue.last_eval_pos,
        self_pos,
    ) {
        return;
    }
    queue.last_eval_pos = self_pos;
    queue.budget_deferred = false;

    if let Some(self_pos) = self_pos {
        queue.pending.make_contiguous().sort_by(|a, b| {
            mmb_load_order_key(a, self_pos).total_cmp(&mmb_load_order_key(b, self_pos))
        });
    }
    let load_radius = gate.draw.world * crate::dat_mzb::MMB_LOAD_DISTANCE_MARGIN;
    let load_radius_sq = load_radius * load_radius;
    // Full-load mode keeps every placement in play instead of holding the far
    // ones back until the view approaches.
    let gate_placements_by_distance = !gate.everything.0;

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
    /// A background stream takes a fraction of the frame's model files.
    const BACKGROUND_SPAWN_BUDGET: usize = 24;
    /// The count budget cannot bound a file carrying dozens of textures, so the
    /// background stream also stops at a wall-clock slice.
    const BACKGROUND_STREAM_SLICE_MS: u128 = 3;
    const HEAVY: usize = 8;
    const MMB_MAX_INFLIGHT: usize = 64;
    let spawn_budget = if background_stream {
        BACKGROUND_SPAWN_BUDGET
    } else {
        MMB_SPAWN_BUDGET
    };
    let frame_started = std::time::Instant::now();
    let mut spawned = 0usize;
    let mut newly_pooled: Vec<u32> = Vec::new();
    let mut retained: std::collections::VecDeque<LoadMmbRequest> =
        std::collections::VecDeque::with_capacity(queue.pending.len());

    while let Some(req) = queue.pending.pop_front() {
        if background_stream
            && spawned > 0
            && frame_started.elapsed().as_millis() >= BACKGROUND_STREAM_SLICE_MS
        {
            queue.budget_deferred = true;
            retained.push_back(req);
            retained.append(&mut queue.pending);
            break;
        }
        if gate_placements_by_distance {
            if let Some(self_pos) = self_pos {
                if is_zone_placement(&req) && mmb_dist_sq_xz(&req, self_pos) > load_radius_sq {
                    retained.push_back(req);
                    retained.append(&mut queue.pending);
                    break;
                }
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

                    if req.world_transform.is_none() && req.static_model.is_none() {
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
                if spawned > 0 && spawned + cost > spawn_budget {
                    queue.budget_deferred = true;
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
                    pool_loaded_textures(loaded, texture_count, quality, &mut images)
                });
                let tex_by_name = &pool.0;
                let first_texture = pool.1.clone();
                if !pool_exists {
                    newly_pooled.push(req.file_id);
                }
                let texture_count = texture_count.max(tex_by_name.len());

                if tracing::enabled!(target: "kuluu_render::dat_mmb", tracing::Level::DEBUG)
                    && mmb_logged.insert((req.file_id, req.chunk_idx))
                {
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
                        target: "kuluu_render::dat_mmb",
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

                if let Some(target) = req.static_model {
                    if commands.get_entity(target.frame).is_ok() {
                        spawn_static_model_parts(
                            &mut commands,
                            &req,
                            target,
                            loaded,
                            &tex_pools_res,
                            &mut handle_cache,
                            &mut meshes,
                            &mut materials,
                            &settings,
                        );
                    }
                    continue;
                }
                if req
                    .entity_id
                    .is_some_and(|id| !tracked.by_id.contains_key(&id))
                {
                    continue;
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
                            e.insert((
                                crate::dat_mzb::AutoMzbOverlay,
                                crate::dat_mzb::ZoneBlockSlot(req.slot),
                            ));
                            if req.sub_area_link != 0 {
                                e.insert(crate::dat_mzb::ZoneSubAreaLink(req.sub_area_link));
                            }
                        }
                        if req.voyage_backdrop {
                            e.insert(crate::transport::VoyageBackdrop(
                                req.world_transform.unwrap_or(Mat4::IDENTITY),
                            ));
                        }
                        if let Some(lod) = req.lod {
                            e.insert(lod);
                        }
                        if let Some(door) = req.door {
                            e.insert(door);
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
                        .or_insert_with(|| meshes.add(submesh_mesh(sub)))
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

                    // Render-state word -> pipeline key (cull / z-bias / depth
                    // write). See FfxiZoneMaterialKey and ffxi-dat MmbRenderState.
                    //
                    // FFXI winding is clockwise (D3D). Mirrored placements
                    // (negative-determinant transforms, ubiquitous for zone
                    // tiles) flip effective winding, so the front-face choice
                    // must ride the pipeline key per placement — xim
                    // GLDrawer.kt drawXim face does the same via glFrontFace.
                    let rs = ffxi_dat::mmb::MmbRenderState::from_blending(sub.blending);
                    let mirrored = req.world_transform.is_some_and(|m| m.determinant() < 0.0);
                    let render_key = crate::ffxi_zone_material::FfxiZoneMaterialKey {
                        back_face_culling: rs.back_face_culling,
                        mirrored,
                        z_bias_level: rs.z_bias_level(),
                        depth_write: rs.depth_write(),
                        // Placed zone geometry is drawn by the ZoneRenderer, not off a
                        // generator, so it takes the single-MODULATE2X terrain chain.
                        generator_stage_chain: false,
                    };

                    // Generator water (sea sheets): force the translucent blend
                    // path and apply the generator tint (alpha < 1). Each water
                    // sheet gets its OWN material (never the shared cache) so its
                    // per-layer UV-scroll animates independently.
                    let mat_handle = if let Some(w) = req.water {
                        materials.add(
                            FfxiZoneMaterial::new(
                                sub_texture,
                                crate::skinned_ffxi_material::FfxiMaterialFlags {
                                    flags: Vec4::new(
                                        has_texture,
                                        1.0,
                                        crate::ffxi_zone_material::ZONE_FLAG_FOGGED,
                                        0.0,
                                    ),
                                },
                                w.tint,
                                Vec4::ZERO,
                                AlphaMode::Blend,
                                crate::ffxi_zone_material::FfxiZoneMaterialKey {
                                    back_face_culling: false,
                                    mirrored,
                                    z_bias_level: 0,
                                    depth_write: false,
                                    generator_stage_chain: true,
                                },
                            )
                            .with_sort_depth_bias(w.sort_bias),
                        )
                    } else {
                        handle_cache
                            .material
                            .entry((
                                cache_key.0,
                                cache_key.1,
                                cache_key.2,
                                mirrored,
                                req.light_bindings,
                                req.area_id,
                            ))
                            .or_insert_with(|| {
                                materials.add(
                                    FfxiZoneMaterial::new(
                                        sub_texture,
                                        crate::skinned_ffxi_material::FfxiMaterialFlags {
                                            flags: Vec4::new(
                                                has_texture,
                                                blend_flag,
                                                crate::ffxi_zone_material::ZONE_FLAG_FOGGED,
                                                discard_threshold,
                                            ),
                                        },
                                        Vec4::ONE,
                                        Vec4::ZERO,
                                        alpha_mode,
                                        render_key,
                                    )
                                    .with_light_bindings(req.light_bindings)
                                    .with_area(req.area_id),
                                )
                            })
                            .clone()
                    };

                    let mut child = commands.spawn((
                        MmbOverlay,
                        Mesh3d(mesh_handle),
                        MeshMaterial3d(mat_handle.clone()),
                        Transform::default(),
                        ChildOf(parent),
                    ));

                    if let Some(door) = req.door {
                        child.insert(crate::zone_doors::ZoneDoorMesh(door));
                    }

                    if let Some(w) = req.water {
                        child.insert((
                            GenWaterScroll {
                                material: mat_handle,
                                uv_scroll: Vec2::from_array([w.uv_scroll.x, w.uv_scroll.y]),
                            },
                            bevy::light::NotShadowCaster,
                            bevy::light::NotShadowReceiver,
                        ));
                    }

                    if is_static_placement {
                        if !settings.zone_shadow_cast {
                            child.insert(bevy::light::NotShadowCaster);
                        }
                        child.insert((
                            crate::components::CameraOccluder,
                            // Static world geometry also renders to the minimap's
                            // offscreen top-down bake camera (RenderLayers::layer(
                            // MINIMAP_BAKE_LAYER)); actors/sky/weather stay on the
                            // default layer only, so the bake captures the zone.
                            bevy::camera::visibility::RenderLayers::default()
                                .with(crate::minimap::topdown::MINIMAP_BAKE_LAYER),
                        ));
                    }

                    child.insert(mesh_debug_bundle(crate::components::MmbDebugInfo {
                        file_id: req.file_id,
                        chunk_idx: req.chunk_idx,
                        sub_index,
                        asset_name: loaded.asset_name.clone(),
                        variant_name: sub.variant_name.trim().to_string(),
                    }));
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
                let at_parse_cap = !in_flight.tasks.contains_key(&asset)
                    && in_flight.tasks.len() >= MMB_MAX_INFLIGHT;
                if at_parse_cap {
                    // The cap defers the parse, not just this frame's pass; without
                    // a repass signal the tail of the queue would never drain.
                    queue.budget_deferred = true;
                }
                if !in_flight.tasks.contains_key(&asset) && in_flight.tasks.len() < MMB_MAX_INFLIGHT
                {
                    let pool = AsyncComputeTaskPool::get();
                    let (file_id, chunk_idx) = (req.file_id, req.chunk_idx);
                    let root_arc = actor_root.0.clone();
                    in_flight.tasks.insert(
                        asset,
                        pool.spawn(async move {
                            let root =
                                crate::ffxi_actor_render::resolve_actor_root(root_arc).ok()?;
                            load_mmb(&root, file_id, chunk_idx).ok()
                        }),
                    );
                }
                retained.push_back(req);
            }
        }
    }
    queue.pending = retained;
    for file_id in newly_pooled {
        drop_pooled_textures(&mut parse_cache, file_id);
    }

    if diag_file_id.is_some() {
        for (fid, examples) in &diag_zero_submesh {
            if examples.is_empty() {
                continue;
            }
            let loaded = diag_loaded.get(fid).copied().unwrap_or(0);
            let load_failed = diag_load_failed.get(fid).copied().unwrap_or(0);
            let head: Vec<&(usize, String)> = examples.iter().take(20).collect();
            info!(
                target: "kuluu_render::dat_mmb::diag",
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
                target: "kuluu_render::dat_mmb::diag",
                file_id = *fid,
                loaded = *loaded,
                load_failed,
                zero_submesh = 0,
                "DIAG-zonegeom MMB pass: all submeshes non-empty",
            );
        }
    }
}

fn submesh_mesh(sub: &MmbSubMesh) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, sub.positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, sub.normals.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, sub.colors.clone());
    mesh.insert_indices(Indices::U32(sub.indices.clone()));
    mesh
}

/// What a static model's parts can bind from what is pooled now: its own file's textures, then every pooled
/// file's in file-id order (ffxi_dat::furniture::match_part_texture).
struct StaticTextureSources {
    own: Vec<(QualifiedTextureName, Handle<Image>)>,
    loaded: Vec<(QualifiedTextureName, Handle<Image>)>,
}

impl StaticTextureSources {
    fn new(pools: &MmbTexPools, file_id: u32) -> Self {
        let sorted = |names: &std::collections::HashMap<QualifiedTextureName, Handle<Image>>| {
            let mut entries: Vec<(QualifiedTextureName, Handle<Image>)> = names
                .iter()
                .map(|(name, handle)| (*name, handle.clone()))
                .collect();
            entries.sort_by_key(|(name, _)| *name);
            entries
        };
        let own = pools
            .by_file
            .get(&file_id)
            .map(|pool| sorted(&pool.2))
            .unwrap_or_default();
        let mut files: Vec<_> = pools.by_file.iter().collect();
        files.sort_by_key(|(id, _)| **id);
        let loaded = files.iter().flat_map(|(_, pool)| sorted(&pool.2)).collect();
        Self { own, loaded }
    }

    fn texture(&self, part: &QualifiedTextureName) -> Option<Handle<Image>> {
        ffxi_dat::furniture::match_part_texture(part, &self.own, &self.loaded)
    }
}

pub fn static_part_texture(
    pools: &MmbTexPools,
    file_id: u32,
    part: &QualifiedTextureName,
) -> Option<Handle<Image>> {
    StaticTextureSources::new(pools, file_id).texture(part)
}

/// The static-model draw state for one part
/// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md A2): the zone pass's
/// lighting and fog, 0x8000 blends the part, 0x2000 draws both faces, and an alpha-tested piece keeps
/// pixels above [`ffxi_dat::furniture::ALPHA_TEST_REF`].
pub fn static_part_material(
    texture: Option<Handle<Image>>,
    blending: u16,
    alpha_test: bool,
    light_bindings: crate::ffxi_zone_material::ZoneLightBindings,
    area_id: u32,
) -> FfxiZoneMaterial {
    let rs = mmb::MmbRenderState::from_blending(blending);
    let threshold = if alpha_test {
        f32::from(ffxi_dat::furniture::ALPHA_TEST_REF) / f32::from(u8::MAX)
    } else {
        0.0
    };
    let alpha_mode = if rs.blend_enabled {
        AlphaMode::Blend
    } else if alpha_test {
        AlphaMode::Mask(threshold)
    } else {
        AlphaMode::Opaque
    };
    let has_texture = if texture.is_some() { 1.0 } else { 0.0 };
    let blend_flag = if rs.blend_enabled { 1.0 } else { 0.0 };
    FfxiZoneMaterial::new(
        texture,
        crate::skinned_ffxi_material::FfxiMaterialFlags {
            flags: Vec4::new(
                has_texture,
                blend_flag,
                crate::ffxi_zone_material::ZONE_FLAG_FOGGED,
                threshold,
            ),
        },
        Vec4::ONE,
        Vec4::ZERO,
        alpha_mode,
        crate::ffxi_zone_material::FfxiZoneMaterialKey {
            back_face_culling: rs.back_face_culling,
            mirrored: false,
            z_bias_level: rs.z_bias_level(),
            depth_write: rs.depth_write(),
            generator_stage_chain: false,
        },
    )
    .with_light_bindings(light_bindings)
    .with_area(area_id)
}

fn spawn_static_model_parts(
    commands: &mut Commands,
    req: &LoadMmbRequest,
    target: StaticModelTarget,
    loaded: &LoadedMmb,
    pools: &MmbTexPools,
    handle_cache: &mut MmbHandleCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiZoneMaterial>,
    settings: &GraphicsSettings,
) {
    let sources = StaticTextureSources::new(pools, req.file_id);
    for (sub_index, sub) in loaded.submeshes.iter().enumerate() {
        let mesh = handle_cache
            .mesh
            .entry((req.file_id, req.chunk_idx, sub_index))
            .or_insert_with(|| meshes.add(submesh_mesh(sub)))
            .clone();
        let texture = sources.texture(&sub.texture_name_raw);
        let textured = texture.is_some();
        let material = materials.add(static_part_material(
            texture,
            sub.blending,
            target.alpha_test,
            req.light_bindings,
            req.area_id,
        ));
        let mut part = commands.spawn((
            MmbOverlay,
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            ChildOf(target.frame),
            StaticModelPart {
                file_id: req.file_id,
                texture_name: sub.texture_name_raw,
                material,
                textured,
            },
        ));
        if !settings.zone_shadow_cast {
            part.insert(bevy::light::NotShadowCaster);
        }
        part.insert(mesh_debug_bundle(crate::components::MmbDebugInfo {
            file_id: req.file_id,
            chunk_idx: req.chunk_idx,
            sub_index,
            asset_name: loaded.asset_name.clone(),
            variant_name: sub.variant_name.trim().to_string(),
        }));
    }
}

/// Retail binds a static-model part to whatever texture is loaded when the piece is built, which in
/// retail is after its room. Here the room's own textures can pool after a piece, so a part that found
/// nothing tries again whenever the pools grow.
pub fn resolve_static_model_textures(
    pools: Res<MmbTexPools>,
    mut parts: Query<&mut StaticModelPart>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
) {
    let mut sources: std::collections::HashMap<u32, StaticTextureSources> = Default::default();
    for mut part in &mut parts {
        if part.textured {
            continue;
        }
        let Some(texture) = sources
            .entry(part.file_id)
            .or_insert_with(|| StaticTextureSources::new(&pools, part.file_id))
            .texture(&part.texture_name)
        else {
            continue;
        };
        if let Some(mut material) = materials.get_mut(&part.material) {
            material.base_color_texture = Some(texture);
            material.material_flags.flags.x = 1.0;
        }
        part.textured = true;
    }
}

/// Choose an MMB submesh's render mode, per XIM (`research/xim` ·
/// `ZoneMeshSection.kt`). The model name (header bytes 16..32, our
/// `zone_mesh_name`) starting with '_' selects an alpha-tested cutout at
/// XIM's `discardThreshold` of 0.375; the `0x8000` flag bit marks translucency
/// (water/glass), rendered as `AlphaMode::Blend` — the zone shader emits real
/// alpha for these (see `flags.y` in `zone_ffxi.wesl`). Everything else is
/// opaque. The render mode is NEVER derived from texture alpha content — doing
/// so punches holes in ordinary opaque ground/wall textures that carry
/// incidental transparency.
fn submesh_alpha_mode(zone_mesh_name: &str, blending: u16, has_texture: bool) -> (AlphaMode, f32) {
    if !has_texture {
        (AlphaMode::Opaque, 0.0)
    } else if zone_mesh_name.starts_with('_') {
        (AlphaMode::Mask(0.375), 0.375)
    } else if mmb::MmbRenderState::from_blending(blending).blend_enabled {
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
/// setting triggered the change. When a pool image's pixel data is already
/// consumed, its asset usage drops to the default first, so the sampler patch
/// re-extracts metadata without asking Bevy to take the data again.
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
        if let Some(mut img) = images.get_mut(handle) {
            if img.data.is_none() {
                img.asset_usage = RenderAssetUsages::default();
            }
            img.sampler = bevy::image::ImageSampler::Descriptor(
                crate::zone_texture::sampler_descriptor(aniso),
            );
        }
    };
    for (by_name, first, by_raw_name) in pools.by_file.values() {
        for handle in by_name.values().chain(by_raw_name.values()) {
            patch(handle);
        }
        if let Some(handle) = first {
            patch(handle);
        }
    }
    applied.anisotropy = Some(aniso);
}

fn mesh_debug_bundle(
    info: crate::components::MmbDebugInfo,
) -> (bevy::picking::Pickable, crate::components::MmbDebugInfo) {
    // IGNORE, not default(): this bundle is attached to every MMB submesh
    // unconditionally, and a blocking Pickable::default() here made MMB props
    // swallow the world-picking ray so entity hitboxes behind them were never
    // hit (kuluu-k929). The off-by-default mesh-debug hover loses MMB hover as a
    // result; it would need its own MeshRayCast to survive opt-in picking.
    (bevy::picking::Pickable::IGNORE, info)
}

#[cfg(test)]
mod tests {
    use super::{
        drop_pooled_textures, mmb_dist_sq_xz, mmb_load_order_key, mmb_repass_needed,
        submesh_alpha_mode, LoadMmbRequest, LoadedMmb, MmbParseCache, NamedTexture,
        MMB_REEVAL_MOVE_YALMS,
    };
    use crate::zone_texture::ffxi_alpha_remap;
    use bevy::prelude::{AlphaMode, Mat4, Vec3};
    use ffxi_dat::mzb::NO_SUB_AREA_LINK;
    use ffxi_dat::texture::{DecodedTexture, TexFormat};

    #[test]
    fn repass_triggers_on_events_parses_budget_or_settings() {
        assert!(mmb_repass_needed(true, false, false, false, None, None));
        assert!(mmb_repass_needed(false, true, false, false, None, None));
        assert!(mmb_repass_needed(false, false, true, false, None, None));
        assert!(mmb_repass_needed(false, false, false, true, None, None));
        assert!(!mmb_repass_needed(false, false, false, false, None, None));
    }

    #[test]
    fn repass_gates_on_reference_position_movement() {
        let prev = Some(Vec3::new(10.0, 0.0, 10.0));
        let near = Some(Vec3::new(10.0 + MMB_REEVAL_MOVE_YALMS * 0.5, 0.0, 10.0));
        let far = Some(Vec3::new(10.0 + MMB_REEVAL_MOVE_YALMS * 2.0, 0.0, 10.0));
        assert!(!mmb_repass_needed(false, false, false, false, prev, prev));
        assert!(!mmb_repass_needed(false, false, false, false, prev, near));
        assert!(mmb_repass_needed(false, false, false, false, prev, far));

        assert!(
            mmb_repass_needed(false, false, false, false, None, prev),
            "self appearing must re-evaluate"
        );
        assert!(
            mmb_repass_needed(false, false, false, false, prev, None),
            "self disappearing must re-evaluate"
        );
    }

    fn zone_placement_at(pos: Vec3) -> LoadMmbRequest {
        LoadMmbRequest {
            area_id: 0,
            light_bindings: Default::default(),
            file_id: 0,
            chunk_idx: 0,
            world_pos: Vec3::ZERO,
            entity_id: None,
            world_transform: Some(Mat4::from_translation(pos)),
            water: None,
            lod: None,
            door: None,
            slot: crate::dat_mzb::ZONE_SLOT_MAIN,
            sub_area_link: NO_SUB_AREA_LINK,
            voyage_backdrop: false,
            static_model: None,
        }
    }

    fn entity_spawn_at(pos: Vec3) -> LoadMmbRequest {
        LoadMmbRequest {
            area_id: 0,
            light_bindings: Default::default(),
            file_id: 0,
            chunk_idx: 0,
            world_pos: pos,
            entity_id: Some(7),
            world_transform: None,
            water: None,
            lod: None,
            door: None,
            slot: crate::dat_mzb::ZONE_SLOT_MAIN,
            sub_area_link: NO_SUB_AREA_LINK,
            voyage_backdrop: false,
            static_model: None,
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

    #[test]
    fn pooled_file_drops_its_decoded_texture_bytes() {
        fn chunk() -> Option<LoadedMmb> {
            Some(LoadedMmb {
                submeshes: Vec::new(),
                textures: vec![NamedTexture {
                    name: "tex".to_string(),
                    raw_name: None,
                    texture: DecodedTexture {
                        width: 1,
                        height: 1,
                        format_tag: TexFormat::Bgra32,
                        rgba: vec![0xFF; 4],
                    },
                }],
                asset_name: "asset".to_string(),
                zone_mesh_name: "mesh".to_string(),
            })
        }

        let mut cache = MmbParseCache::default();
        cache.by_asset.insert((10, 0), chunk());
        cache.by_asset.insert((10, 1), chunk());
        cache.by_asset.insert((11, 0), chunk());

        drop_pooled_textures(&mut cache, 10);

        for idx in [0usize, 1] {
            let entry = cache.by_asset[&(10, idx)].as_ref().expect("cached chunk");
            assert!(
                entry.textures.is_empty(),
                "a pooled file's chunks must not keep a duplicate of its decoded texels"
            );
        }
        assert_eq!(
            cache.by_asset[&(11, 0)]
                .as_ref()
                .expect("cached chunk")
                .textures
                .len(),
            1,
            "an unpooled file must keep its texels: it has no uploaded handles yet"
        );
    }
}

#[cfg(test)]
mod zone_shadow_cast_tests {
    use super::*;
    use bevy::light::NotShadowCaster;

    fn app_with(zone_shadow_cast: bool) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.insert_resource(GraphicsSettings {
            zone_shadow_cast,
            ..GraphicsSettings::default()
        });
        app.add_systems(Update, apply_zone_shadow_cast);
        let placement = app
            .world_mut()
            .spawn((MmbOverlay, crate::components::CameraOccluder))
            .id();
        let actor_model = app.world_mut().spawn(MmbOverlay).id();
        (app, placement, actor_model)
    }

    #[test]
    fn placements_stay_out_of_the_shadow_pass_by_default() {
        let (mut app, placement, actor_model) = app_with(false);
        app.update();
        assert!(app.world().get::<NotShadowCaster>(placement).is_some());
        assert!(
            app.world().get::<NotShadowCaster>(actor_model).is_none(),
            "only static placements are gated; models on actors keep their own rule"
        );
    }

    #[test]
    fn opting_in_and_out_flips_existing_placements() {
        let (mut app, placement, _) = app_with(false);
        app.update();
        app.world_mut()
            .resource_mut::<GraphicsSettings>()
            .zone_shadow_cast = true;
        app.update();
        assert!(app.world().get::<NotShadowCaster>(placement).is_none());
        app.world_mut()
            .resource_mut::<GraphicsSettings>()
            .zone_shadow_cast = false;
        app.update();
        assert!(app.world().get::<NotShadowCaster>(placement).is_some());
    }
}

#[cfg(test)]
mod static_model_tests {
    use super::*;
    use crate::ffxi_zone_material::ZoneLightBindings;
    use ffxi_dat::texture::NAMESPACE_LEN;

    const BLEND: u16 = 0x8000;
    const BOTH_FACES: u16 = 0x2000;
    const OWN_FILE: u32 = 7;
    const OTHER_FILE: u32 = 3;
    const LATER_FILE: u32 = 9;

    fn name(namespace: &[u8; 8], local: &[u8; 8]) -> QualifiedTextureName {
        let mut n = [0u8; 16];
        n[..NAMESPACE_LEN].copy_from_slice(namespace);
        n[NAMESPACE_LEN..].copy_from_slice(local);
        n
    }

    fn pool(
        names: &[(QualifiedTextureName, Handle<Image>)],
    ) -> (
        std::collections::HashMap<String, Handle<Image>>,
        Option<Handle<Image>>,
        std::collections::HashMap<QualifiedTextureName, Handle<Image>>,
    ) {
        (
            Default::default(),
            names.first().map(|(_, h)| h.clone()),
            names.iter().cloned().collect(),
        )
    }

    #[test]
    fn a_static_part_blends_on_0x8000_and_alpha_tests_only_when_the_piece_does() {
        let bindings = ZoneLightBindings::default();
        let blended = static_part_material(None, BLEND, false, bindings, 0);
        assert_eq!(blended.alpha_mode, AlphaMode::Blend);
        assert_eq!(blended.material_flags.flags.y, 1.0);
        assert_eq!(blended.material_flags.flags.w, 0.0);
        assert!(!blended.render_key.depth_write);

        let cutout = static_part_material(None, 0, true, bindings, 0);
        let threshold = f32::from(ffxi_dat::furniture::ALPHA_TEST_REF) / f32::from(u8::MAX);
        assert_eq!(cutout.alpha_mode, AlphaMode::Mask(threshold));
        assert_eq!(cutout.material_flags.flags.w, threshold);
        assert!(cutout.render_key.back_face_culling);
        assert!(!cutout.render_key.mirrored && !cutout.render_key.generator_stage_chain);

        let blended_cutout = static_part_material(None, BLEND, true, bindings, 0);
        assert_eq!(blended_cutout.alpha_mode, AlphaMode::Blend);
        assert_eq!(blended_cutout.material_flags.flags.w, threshold);

        let two_sided = static_part_material(None, BOTH_FACES, false, bindings, 0);
        assert_eq!(two_sided.alpha_mode, AlphaMode::Opaque);
        assert!(!two_sided.render_key.back_face_culling);
        assert_eq!(two_sided.material_flags.flags.x, 0.0, "no texture bound");
    }

    #[test]
    fn a_static_part_takes_its_own_file_then_the_full_name_then_the_local_name() {
        let mut images = Assets::<Image>::default();
        let own = images.add(Image::default());
        let full = images.add(Image::default());
        let local = images.add(Image::default());
        let part = name(b"furnitur", b"desk_top");
        let mut pools = MmbTexPools::default();
        pools.by_file.insert(
            OTHER_FILE,
            pool(&[(name(b"zone____", b"desk_top"), local.clone())]),
        );
        pools
            .by_file
            .insert(LATER_FILE, pool(&[(part, full.clone())]));
        pools.by_file.insert(OWN_FILE, pool(&[(part, own.clone())]));
        assert_eq!(static_part_texture(&pools, OWN_FILE, &part), Some(own));
        pools.by_file.remove(&OWN_FILE);
        assert_eq!(static_part_texture(&pools, OWN_FILE, &part), Some(full));
        pools.by_file.remove(&LATER_FILE);
        assert_eq!(static_part_texture(&pools, OWN_FILE, &part), Some(local));
        pools.by_file.clear();
        assert_eq!(static_part_texture(&pools, OWN_FILE, &part), None);
    }

    #[test]
    fn an_untextured_part_binds_a_texture_that_pools_later() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<FfxiZoneMaterial>>()
            .init_resource::<MmbTexPools>()
            .add_systems(Update, resolve_static_model_textures);
        let part_name = name(b"furnitur", b"desk_top");
        let material = app
            .world_mut()
            .resource_mut::<Assets<FfxiZoneMaterial>>()
            .add(static_part_material(None, 0, false, Default::default(), 0));
        let part = app
            .world_mut()
            .spawn(StaticModelPart {
                file_id: OWN_FILE,
                texture_name: part_name,
                material: material.clone(),
                textured: false,
            })
            .id();
        app.update();
        assert!(!app.world().get::<StaticModelPart>(part).unwrap().textured);

        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.world_mut()
            .resource_mut::<MmbTexPools>()
            .by_file
            .insert(OTHER_FILE, pool(&[(part_name, image.clone())]));
        app.update();
        assert!(app.world().get::<StaticModelPart>(part).unwrap().textured);
        let materials = app.world().resource::<Assets<FfxiZoneMaterial>>();
        let bound = materials.get(&material).unwrap();
        assert_eq!(bound.base_color_texture, Some(image));
        assert_eq!(bound.material_flags.flags.x, 1.0);
    }
}
