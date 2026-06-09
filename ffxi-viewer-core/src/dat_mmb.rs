//! MMB debug overlay: load a real FFXI entity model from a DAT file
//! (resolved via `ffxi-dat::DatRoot::from_env_or_default`) and spawn
//! it as a Bevy mesh at a chosen world position.
//!
//! The `_or_default` variant means the workspace `cargo run` path
//! works without anyone setting `FFXI_DAT_PATH` — it falls back to
//! `vendor/Game/SquareEnix/FINAL FANTASY XI`. Installed-binary users
//! still need to set the env var explicitly (the fallback resolves
//! relative to CWD, not relative to the executable).
//!
//! Plumbed via [`LoadMmbRequest`] events — the slash-command dispatcher
//! in `ffxi-client::view_native::text_input` fires the event; the
//! [`process_load_mmb_requests`] system consumes it. Keeps the text
//! input system from having to take direct asset-storage params.
//!
//! Native-only: `ffxi-dat` does synchronous `fs::read` of the user's
//! local install, which has no equivalent on wasm32. The browser viewer
//! can grow a parallel `LoadMmbHttp` path later if needed.
//!
//! See `ffxi-viewer-core/examples/mmb-view.rs` for a standalone
//! orbit-camera renderer of the same pipeline.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::mmb::{parse_models, MmbHeader};
use ffxi_dat::texture::{decode_texture, DecodedTexture};
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

use crate::look_resolver::dispatch_look_driven_models;
use crate::scene::TrackedEntities;

/// Marker for overlay entities spawned by this module — lets the
/// `/load_mmb clear` command (future work) find and despawn them.
#[derive(Component)]
pub struct MmbOverlay;

/// Persistent (across frames) cache of MMB submesh `Handle<Mesh>` +
/// `Handle<StandardMaterial>`. Keyed by `(file_id, chunk_idx, sub_index)`.
///
/// Two adjacent identical trees in Ronfaure spawn as separate Bevy
/// entities, but Bevy's automatic GPU-instancing batcher will collapse
/// them into one `multi_draw_indirect` call iff they share both the
/// mesh handle and the material handle (and the pipeline). The
/// previous in-function HashMap was correct but only lived for one
/// invocation of `process_load_mmb_requests`; chunks that arrive
/// across multiple frames (incremental zone load) bypassed the cache.
/// This Resource lifts it to app-wide lifetime so the batcher sees
/// matching handles regardless of when each instance was queued.
///
/// Eviction: zone change wipes the cache via the OnExit hook for
/// `AppPhase::InGame` (no current consumer outside in-game; the
/// alternative was hooking each `AutoMzbOverlay` despawn, which would
/// require ref-counting). The cache cost is bounded by the unique
/// submesh count of the loaded MMBs (~thousands of asset entries —
/// kilobytes of HashMap), so we err toward keeping it warm.
#[derive(Resource, Default)]
pub struct MmbHandleCache {
    pub mesh: std::collections::HashMap<(u32, usize, usize), bevy::asset::Handle<Mesh>>,
    pub material:
        std::collections::HashMap<(u32, usize, usize), bevy::asset::Handle<StandardMaterial>>,
}

/// Cross-frame backlog of pending [`LoadMmbRequest`]s. A city zone-in
/// fires ~1000+ requests in one frame; draining them all at once locks
/// the main thread for hundreds of ms. [`process_load_mmb_requests`]
/// instead enqueues here and drains a bounded budget per frame, so the
/// zone fills in progressively over a handful of frames. Cleared on
/// `OnExit(AppPhase::InGame)` so a re-zone doesn't pop the old zone's
/// backlog into the new one.
#[derive(Resource, Default)]
pub struct MmbLoadQueue {
    pub pending: std::collections::VecDeque<LoadMmbRequest>,
}

/// One parsed [`LoadedMmb`] per `(file_id, chunk_idx)`, persisted across
/// frames. Lifting the former per-frame cache to app lifetime preserves
/// the per-asset parse-once dedup now that draining spans frames — a
/// request popped in frame N+5 for a file parsed in frame N is still a
/// cache hit. `None` marks a load that failed (don't retry every pop).
#[derive(Resource, Default)]
pub struct MmbParseCache {
    pub by_asset: std::collections::HashMap<(u32, usize), Option<LoadedMmb>>,
}

/// Per-DAT decoded-texture pool (`name → image handle`, plus a first-IMG
/// fallback), persisted across frames for the same reason as
/// [`MmbParseCache`] — build the IMG pool once per `file_id` regardless
/// of which frame each MMB from that file is drained.
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

/// Spawn-an-MMB request. Fired by the slash-command dispatcher;
/// consumed by [`process_load_mmb_requests`].
///
/// When `entity_id` is `Some`, the spawned mesh is parented under the
/// `WorldEntity` with that wire id (looked up via `TrackedEntities`) —
/// the model then moves with the entity. When `None`, the mesh spawns
/// as a free overlay at `world_pos` (original `/load_mmb` behaviour).
///
/// `world_pos` is already in Bevy coordinates — the parser pre-applies
/// `ffxi_to_bevy` so this system stays unaware of the FFXI/Bevy axis
/// convention. When `entity_id` is `Some`, `world_pos` is ignored.
#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMmbRequest {
    pub file_id: u32,
    pub chunk_idx: usize,
    /// Bevy-space translation for the spawned MMB parent. Ignored when
    /// `entity_id` is `Some` (the mesh inherits the tracked entity's
    /// transform instead) or when `world_transform` is `Some` (the
    /// full matrix wins). Kept as the simple/legacy form for
    /// `/load_mmb` and entity-look spawns.
    pub world_pos: Vec3,
    pub entity_id: Option<u32>,
    /// Full Bevy-space placement transform. `Some` when the spawn
    /// comes from an MZB placement record — already includes the
    /// FFXI→Bevy axis flip composed with the FFXI-native
    /// trans/rot/scale of the `SMZBBlock100`. The MMB local-space
    /// vertices stay in FFXI-native coords; this transform does the
    /// flip-and-place in one matrix.
    pub world_transform: Option<Mat4>,
}

/// Plugin: registers the MMB and MZB debug-overlay events and their
/// consumer systems. Added by `ViewerCorePlugin` so both front-ends
/// pick it up (the wasm cfg gate at the top of this file means the
/// whole module is absent on wasm32, and the lib.rs plugin add is
/// gated the same way).
pub struct DatOverlayPlugin;

impl Plugin for DatOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LoadMmbRequest>()
            .add_message::<crate::dat_vos2::LoadVos2Request>()
            .add_message::<crate::dat_mzb::LoadMzbRequest>()
            .init_resource::<MmbHandleCache>()
            .init_resource::<MmbLoadQueue>()
            .init_resource::<MmbParseCache>()
            .init_resource::<MmbTexPools>()
            .init_resource::<crate::dat_mzb::LastAutoLoadedZone>()
            .init_resource::<crate::dat_mzb::DrawDistance>()
            .init_resource::<crate::dat_mzb::MzbCollisionGeometry>()
            .init_resource::<crate::dat_mzb::LoadMzbInFlight>()
            .init_resource::<crate::dat_mzb::ZoneGeomCache>()
            .add_systems(
                Update,
                (
                    // Order matters: zone-change watcher writes a
                    // `LoadMzbRequest` that the consumer system reads
                    // *the same frame* — chaining the pair removes the
                    // one-frame delay that an unchained order would
                    // introduce on every zone transition. The look
                    // dispatcher must run *before* `process_load_mmb_requests`
                    // so the events it emits get consumed the same
                    // frame the look change was detected — otherwise
                    // an entity's model appears one tick after its
                    // look update arrives.
                    crate::dat_mzb::auto_load_zone_geometry_system,
                    dispatch_look_driven_models,
                    // MZB load runs before the MMB consumer so that the
                    // zone-MMB spawn list it emits (one LoadMmbRequest
                    // per placement record) gets consumed in the same
                    // frame the zone changes.
                    //
                    // Phase A: parse + bake now runs on a background
                    // `AsyncComputeTaskPool` task. `kick_load_mzb_tasks`
                    // drains the event into either a cache hit (spawn
                    // this frame) or a new task. `poll_load_mzb_tasks`
                    // runs immediately after so a hit-or-fast-task can
                    // still fire its `LoadMmbRequest`s within the same
                    // frame the zone changed. Slow zones (large city
                    // DATs) take one or more frames to land, with no
                    // main-thread blocking in the meantime.
                    crate::dat_mzb::kick_load_mzb_tasks,
                    crate::dat_mzb::poll_load_mzb_tasks,
                    process_load_mmb_requests,
                    crate::dat_vos2::process_load_vos2_requests,
                )
                    .chain(),
            )
            // `/drawdistance setmob` consumer — culls non-PC entities
            // outside the configured radius. Stays decoupled from the
            // MZB load chain so it runs every frame independent of the
            // (rare) zone-change events.
            .add_systems(
                Update,
                (
                    crate::dat_mzb::cull_entities_by_distance,
                    crate::dat_mzb::apply_zone_geom_visibility,
                    // `tick_skinned_actors` is registered in `lib.rs`
                    // alongside its `combat_stance::track_entity_motion_system`
                    // `.before(...)` ordering constraint — don't add it
                    // here too or `.before(tick_skinned_actors)` becomes
                    // an ambiguous SystemTypeSet and Bevy panics at
                    // schedule init.
                ),
            );
        // Phase 1 `cull_mzb_by_distance` was removed: Phase 3 merged
        // everything into two entities anchored at world origin, so
        // the per-entity distance check would hide the whole zone
        // once the player walks >80 yalms from origin.
    }
}

/// Pure-data representation of one MMB sub-record, ready to bake into
/// a Bevy `Mesh`. Lives between [`load_mmb`] (parse) and the spawn
/// step so the parse half stays testable without an `App`.
pub struct MmbSubMesh {
    pub variant_name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Raw blending field from `SMMBModelHeader.blending`. Bit 0x8000
    /// marks alpha-blended (translucent) geometry in lotus's pipeline
    /// (`mesh->has_transparency = mmb_mesh.blending & 0x8000`). We
    /// only check that bit at material-build time; the rest of the
    /// field's bit-meaning is undocumented.
    pub blending: u16,
}

/// One named IMG chunk decoded into RGBA. The `name` is the 8-byte
/// internal asset name from the IMG body (e.g. `"s_kabe2"`); it's what
/// MMB sub-records' `variant_name` field references.
#[derive(Debug, Clone)]
pub struct NamedTexture {
    pub name: String,
    pub texture: DecodedTexture,
}

/// One MMB load → meshes + a pool of textures from IMG chunks in the
/// same DAT file. Texture-to-submesh binding happens at spawn time by
/// matching `submesh.variant_name` to `NamedTexture.name`.
pub struct LoadedMmb {
    pub submeshes: Vec<MmbSubMesh>,
    pub textures: Vec<NamedTexture>,
    /// MMB header's `asset_name` field — the 16-byte ASCII name the
    /// MZB placement table looks up. Useful for mouse-over debug HUDs
    /// that identify which placement-table entry a mesh came from.
    pub asset_name: String,
}

/// Load + decrypt + parse an MMB at the given file_id / chunk_idx.
/// Returns one [`MmbSubMesh`] per sub-record that has both vertices
/// and triangles plus any decodable [`DecodedTexture`]s pulled from
/// IMG chunks colocated in the same DAT file. Sub-records and IMGs
/// that fail their respective checks are skipped silently — the
/// caller sees how many came back and can warn the operator if the
/// count is zero.
///
/// All errors are flattened to `String` so the chat HUD can display
/// them. The underlying `ffxi_dat::DatError` already implements
/// `Display`, so the formatter does the work.
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
    // Structural walk (lotus-parity). The previous heuristic scanner
    // (`MmbSubRecord::find_all`) found ASCII-looking 16-byte windows
    // and missed real per-submesh records embedded in city assets —
    // verified live against tshimonorig_06: scanner returned 2 of
    // many submeshes, the structural walker returns all of them.
    let models = parse_models(&decrypted);

    // Scrape IMG chunks from the same DAT. Many files have dozens of
    // IMGs (file 200 = 53; file 133 = 47). Each model's 8-byte texture
    // name (from the last half of `SMMBModelHeader.textureName`) is
    // paired against `NamedTexture.name` at spawn time; unmatched
    // submeshes fall back to the first decodable IMG.
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
        // Sanity check: real FFXI zone props are well within ±10000
        // yards of origin. The structural walker already chooses the
        // right vertex stride (36 vs 48), so this should almost never
        // fire — kept as a defense against malformed/truncated DATs.
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
        // FFXI vertex colors use the "0x80 = 1.0" working range
        // convention (lotus mmb.cppm:342-343 divides by 128 for all
        // channels). Byte 128 = fully lit, byte 255 = "overbright"
        // up to 2x. We previously divided by 255, halving FFXI's
        // intended brightness on every MMB — visible as the dim/dark
        // feel in earlier zone renders. It also pushed vertex alpha
        // into the 0..0.5 range, which multiplied with our
        // remapped-to-binary texture alpha (1.0) put `Mask(0.5)`
        // right at its threshold and discarded most leaf pixels.
        //
        // Switch all channels to /128 to match lotus. HDR + TonyMcMapface
        // tonemapping handles the >1.0 values gracefully (gives nice
        // highlights instead of clipping).
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
        // Defense-in-depth bounds-check against the vertex array.
        // `parse_models` produces well-formed indices, but a truncated
        // DAT could still feed us bad bytes.
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
    Ok(LoadedMmb {
        submeshes: out,
        textures,
        asset_name,
    })
}

// Earlier alpha-probing attempts (any α<255; ≥1% pixels with α<16)
// produced dithered-checkerboard rendering on FFXI textures because
// FFXI authors alpha in a 0..128 working range ("0x80 = 1.0"
// convention, same as its vertex-color decode). Empirical data from
// Ronfaure (DAT 200 chunk 1165, `ron_wf`) shows alpha ranges like
// [0..136] — not [0..255]. Lotus's MMB shader (mmb.slang::FragmentBlend)
// remaps this on the fly: `bc2_alpha = raw >> 4`,
// `output.alpha = bc2_alpha / 8.0` (clamped to 1.0). We bake the
// equivalent remap into the texture at decode time below.

/// Lotus-parity alpha remap. Input is the raw FFXI alpha byte;
/// output is the value Bevy's `AlphaMode::Blend` should use as the
/// blend factor.
///
/// Formula (lotus mmb.slang:107-110):
///   `bc2_alpha = raw >> 4`       (extract 4-bit value, 0..15)
///   `out_float = bc2_alpha / 8.0` (range 0..1.875, clamped to 1.0)
///   `out_byte  = round(out_float * 255)`
///
/// Producing the table:
///   raw 0..15   → 0   (lotus: discard; we'll let Blend handle it)
///   raw 16..31  → 32
///   raw 32..47  → 64
///   raw 48..63  → 96
///   raw 64..79  → 128
///   raw 80..95  → 160
///   raw 96..111 → 191
///   raw 112..127→ 223
///   raw 128..255→ 255 (clamped)
#[inline]
fn ffxi_alpha_remap(raw: u8) -> u8 {
    // Float math (not integer) so we match lotus's `bc2_alpha / 8.0`
    // exactly — integer division of `(1 * 255) / 8` yields 31, but
    // lotus's float math `1/8 * 255 = 31.875` rounds to 32.
    let bc2 = (raw >> 4) as f32; // 0..15
    let scaled = bc2 * 255.0 / 8.0;
    scaled.min(255.0).round() as u8
}

/// Convert a [`DecodedTexture`] into a Bevy [`Image`] asset. The
/// texture decoder produces top-mip RGBA8 already; we just wrap it in
/// the asset type Bevy expects for `base_color_texture` after
/// remapping alpha bytes per [`ffxi_alpha_remap`].
fn decoded_texture_to_image(t: &DecodedTexture) -> Image {
    // Apply lotus's alpha remap. Opaque-mode submeshes ignore alpha
    // entirely, so for them this is harmless. Blend-mode submeshes
    // get correctly-scaled opacity (raw 128+ → fully opaque leaf
    // body, raw 0 → fully transparent hole).
    let mut rgba = t.rgba.clone();
    for px in rgba.chunks_exact_mut(4) {
        px[3] = ffxi_alpha_remap(px[3]);
    }
    let mut img = Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        // sRGB — FFXI textures are authored for the gamma-encoded color
        // pipeline; using `Rgba8UnormSrgb` lets Bevy linearize correctly
        // for PBR lighting.
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    // FFXI textures are authored for tiling: walls/floors carry UVs that
    // run 0..N over a multi-tile surface. The Bevy default `ClampToEdge`
    // stretches the last texel across the rest of the surface — that's
    // the vertical-stripe banding visible on tall columns. `Repeat`
    // wraps correctly. Nearest mag-filter keeps the retail "crisp pixel"
    // look on close-up textures while linear min/mipmap stays anti-
    // aliased at distance.
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        address_mode_w: bevy::image::ImageAddressMode::Repeat,
        mag_filter: bevy::image::ImageFilterMode::Nearest,
        min_filter: bevy::image::ImageFilterMode::Linear,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        ..Default::default()
    });
    img
}

// Stage 5a removed the 6-color debug palette here. Submeshes now
// modulate through `Mesh::ATTRIBUTE_COLOR` (FFXI's per-vertex RGBA,
// which encodes pre-baked diffuse/ambient lighting in the original
// client). The StandardMaterial keeps `base_color = WHITE`, and Bevy
// multiplies vertex colors through automatically as long as the mesh
// carries the color attribute and the material is built fresh — so
// every submesh of every MMB shows its real per-vertex shading
// instead of an arbitrary per-index color.
//
// Real diffuse textures (Stage 5b) layer on top via `base_color_texture`
// when an IMG chunk is paired to the MMB inside `process_load_mmb_requests`.

/// Consume [`LoadMmbRequest`] events: load the MMB and spawn one Bevy
/// mesh entity per sub-record under a parent transform at `world_pos`.
/// Failures get pushed into the scene's system chat buffer so the
/// operator sees why nothing showed up.
pub fn process_load_mmb_requests(
    mut events: MessageReader<LoadMmbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    tracked: Res<TrackedEntities>,
    mut handle_cache: ResMut<MmbHandleCache>,
    mut queue: ResMut<MmbLoadQueue>,
    mut parse_cache: ResMut<MmbParseCache>,
    mut tex_pools_res: ResMut<MmbTexPools>,
) {
    // Enqueue this frame's new requests at the back of the backlog. A
    // zone-in for a city like Bastok Markets fires ~1000+ events that
    // all hit one file_id; draining them all in one frame is the CPU
    // lock we're avoiding, so we spread the work across frames below.
    queue.pending.extend(events.read().copied());
    if queue.pending.is_empty() {
        return;
    }

    // Per-frame dedup for the "MMB texture pool" diagnostic below. Each
    // unique asset is popped exactly once across all frames, so a
    // frame-local set still logs each asset ~once — no need to persist.
    let mut mmb_logged: std::collections::HashSet<(u32, usize)> = std::collections::HashSet::new();

    // DIAG-zonegeom: remove after fix. Aggregator for the sibling
    // diagnostic in build_zone_mmb_spawns — counts and exemplars of
    // zero-submesh MMBs per file_id (cause (B) candidates: clod-style
    // sub-records mis-parsed). Gated on `FFXI_DIAG_ZONE_GEOM` exactly
    // like the MZB-side diag (file_id direct, or `=all`/`=*`/`=any`).
    // Now reports per-frame slices since draining spans frames.
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

    // Weighted per-frame budget. A cold parse (cache miss → fs::read +
    // XOR-decrypt + parse + IMG decode) costs `HEAVY`; a warm cache-hit
    // spawn costs 1. So a frame does ~6 cold loads OR ~48 warm spawns,
    // and a ~1000-MMB city zone fills in over ~20 frames (~0.3 s @60fps)
    // with no single-frame hitch. A count budget (not a wall-clock
    // `Instant`) because the per-file cache makes per-request cost
    // bimodal — a count is predictable and testable, where a time
    // budget would make the spawn count frame-rate-dependent.
    const MMB_SPAWN_BUDGET: usize = 48;
    const HEAVY: usize = 8;
    let mut work = 0usize;

    while work < MMB_SPAWN_BUDGET {
        let Some(req) = queue.pending.pop_front() else {
            break;
        };
        // Count the work before any early-`continue` so a cold load that
        // fails or yields zero submeshes still draws down the budget.
        let was_cached = parse_cache
            .by_asset
            .contains_key(&(req.file_id, req.chunk_idx));
        work += if was_cached { 1 } else { HEAVY };
        let loaded_entry = parse_cache
            .by_asset
            .entry((req.file_id, req.chunk_idx))
            .or_insert_with(|| load_mmb(req.file_id, req.chunk_idx).ok());
        let Some(loaded) = loaded_entry.as_ref() else {
            push_system_msg(
                &mut toasts,
                format!("/load_mmb {} {}: load failed", req.file_id, req.chunk_idx),
            );
            // DIAG-zonegeom: remove after fix.
            if diag_matches(req.file_id) {
                *diag_load_failed.entry(req.file_id).or_insert(0) += 1;
            }
            continue;
        };

        // DIAG-zonegeom: remove after fix.
        if diag_matches(req.file_id) {
            *diag_loaded.entry(req.file_id).or_insert(0) += 1;
        }

        if loaded.submeshes.is_empty() {
            // DIAG-zonegeom: remove after fix. Capture cause (B)
            // candidates: zone-spawn MMBs (world_transform is Some)
            // that produce zero sub-records get silently skipped in
            // the chat HUD path below — log them here so we see them.
            if diag_matches(req.file_id) {
                diag_zero_submesh
                    .entry(req.file_id)
                    .or_default()
                    .push((req.chunk_idx, loaded.asset_name.clone()));
            }
            // Suppress for zone-spawn events (req.world_transform is
            // Some). Hundreds of MMBs in a city zone are clod-style
            // sub-records we don't decode yet (task #18); spamming
            // chat for each one drowns out actual operator messages.
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

        // Build a name → image-handle pool once per file_id. Each
        // submesh's `variant_name` (e.g. `"s_kabe2"`) matches the IMG
        // body's internal name (`extract_texture_name`). Submeshes that
        // don't match fall back to the first IMG or no texture.
        let texture_count = loaded.textures.len();
        let pool = tex_pools_res.by_file.entry(req.file_id).or_insert_with(|| {
            let mut by_name: std::collections::HashMap<String, Handle<Image>> =
                std::collections::HashMap::with_capacity(texture_count);
            let mut first: Option<Handle<Image>> = None;
            for nt in &loaded.textures {
                let handle = images.add(decoded_texture_to_image(&nt.texture));
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

        // Per-MMB diagnostic: log once per (file_id, chunk_idx) per
        // frame so we can see blending values + texture alpha ranges
        // for every distinct asset. Most zone loads only fire the
        // first time the player zones in; the per-frame retry is a
        // wash because mmb_cache deduplicates loads.
        // Enable with `RUST_LOG=ffxi_viewer_core::dat_mmb=info`.
        if mmb_logged.insert((req.file_id, req.chunk_idx)) {
            // `SMMBHeader.pieces` (lotus mmb.cppm:98) sits at the first
            // 4 bytes of the payload — i.e. decrypted bytes 32..36 from
            // the file. We don't yet decode the block headers, so we
            // just probe `pieces` to compare against what our heuristic
            // scanner actually returned. A mismatch (pieces > 0 but
            // we see far fewer than `numModel * pieces` submeshes)
            // tells us the scanner is missing structural records.
            // Alpha range per IMG texture: lets us see whether a
            // transparency-flagged submesh's texture actually carries
            // varying alpha (real cutout art) or is flat all-255
            // (the architectural limit case — even lotus's FragmentBlend
            // would render this opaque).
            //
            // Format: ["texname α[min..max]", …] sorted by name.
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
            // Per-submesh blending dump — used to verify whether tree-
            // leaf and similar foliage submeshes have `0x8000` set
            // (which would trigger our AlphaMode::Blend path) or not
            // (in which case lotus also renders them as opaque
            // rectangles and we'd need to replicate its
            // mmb.slang::FragmentBlend logic in a custom material).
            //
            // Format: ["texname:0xBLEND", ...] — sorted by texname so
            // grep-friendly across runs.
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

        // Two parenting modes:
        //
        // - `entity_id = Some(id)` (look-driven or `/load_mmb_on`): hang
        //   the meshes under the existing `WorldEntity` so they inherit
        //   its world transform, and strip the entity's debug capsule
        //   `Mesh3d` so the real model replaces the placeholder.
        // - `entity_id = None` (free overlay, original `/load_mmb`):
        //   spawn a new parent at `world_pos`.
        // `true` when the spawn produces a new static parent (zone
        // placement or free `/load_mmb` overlay) rather than attaching
        // meshes under a moving `WorldEntity`. Static placements should
        // participate in camera occlusion; entity-attached models
        // (NPCs, PCs, pets) should not — they're small, move every
        // frame, and would force a BVH rebuild storm.
        let is_static_placement = req
            .entity_id
            .and_then(|id| tracked.by_id.get(&id))
            .is_none();
        let parent = match req.entity_id.and_then(|id| tracked.by_id.get(&id)) {
            Some(&bevy_e) => {
                // Hide the debug capsule by removing its mesh handle.
                // We don't despawn the WorldEntity itself — it carries
                // the wire id, transform, picking, nameplate, and HP bar
                // child, all of which we still want.
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
                let is_zone_spawn = req.entity_id.is_none() && req.world_transform.is_some();
                // Tag with `InGameEntity` so `OnExit(AppPhase::InGame)`
                // recursively despawns this parent and every submesh
                // child below it. Without the marker, zone-spawned
                // props (textured buildings, walls, foliage — the bulk
                // of what you see in a city zone) and free `/load_mmb`
                // overlays survived /logout and stayed rendered behind
                // the launcher.
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

        // Cloud mesh: FFXI tags the sky cloud MMB with the header name
        // "clod" (cite: lotus landscape_entity, RZN FFXILandscapeMesh).
        // Mark the parent so `zone_clouds` can gate its visibility by sky
        // style (Retail shows this authored mesh; Enhanced uses the
        // procedural dome) and drift it slowly. Detection is name-based
        // on the already-decoded `asset_name` — no extra DAT parsing.
        if loaded
            .asset_name
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("clod")
        {
            commands
                .entity(parent)
                .insert(crate::zone_clouds::CloudMesh);
        }

        let n_subs = loaded.submeshes.len();
        for (sub_index, sub) in loaded.submeshes.iter().enumerate() {
            let cache_key = (req.file_id, req.chunk_idx, sub_index);
            // Mesh handle: reuse if already built. Vertex data is
            // identical across instances; only the parent transform
            // differs (and that's per-entity, not per-mesh).
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

            // Alpha mode is driven by `SMMBModelHeader.blending`:
            //   any non-zero blending bit → AlphaMode::Mask(0.5)
            //   blending == 0             → AlphaMode::Opaque
            //
            // Why "any non-zero" and not "& 0x8000" (which is lotus's
            // check at mmb.cppm:501): empirically, FFXI uses multiple
            // bits in this field. Ronfaure trees (asset
            // `tshimonoyama_1_m`) flag leaves as `0x8000`, but San
            // d'Oria plants (asset `tshimono_plant_03`) flag leaves
            // as `0x2000`. Both are foliage textures with cutout
            // intent. Lotus's narrow check means it would render the
            // plant opaque too — we can do better by recognising any
            // non-zero blending as a transparency signal.
            //
            // We use *Mask*, not Blend, because FFXI authors the
            // "transparent" regions of leaf/foliage textures with
            // dark RGB. Bevy's Blend mode multiplies `src.rgb *
            // src.alpha + dst * (1-α)`; at α≈0 the dark src still
            // contributes ~0 (correct), but mipmap-blended edge
            // pixels with α≈0.5 contribute ~50% of their dark RGB,
            // producing visible black outlines around leaf
            // silhouettes. Mask discards below-threshold pixels
            // entirely so those dark pixels never reach the
            // framebuffer.
            //
            // The remap from `ffxi_alpha_remap` (lotus's bc2 / 8.0
            // formula) pushes typical foliage textures with raw
            // α[0..136] into a near-binary 0/255 distribution, so
            // threshold 0.5 cleanly separates leaf from hole.
            let alpha_mode = if sub.blending != 0 {
                AlphaMode::Mask(0.5)
            } else {
                AlphaMode::Opaque
            };
            // Material handle: reuse the same StandardMaterial across
            // instances. The material depends only on
            // (texture, alpha_mode) which is a function of
            // `(file_id, chunk_idx, sub_index)`.
            let mat_handle = handle_cache
                .material
                .entry(cache_key)
                .or_insert_with(|| {
                    materials.add(StandardMaterial {
                        // WHITE so the mesh's per-vertex `ATTRIBUTE_COLOR`
                        // (FFXI's baked vertex lighting) and the bound
                        // `base_color_texture` (if any) both pass through
                        // un-tinted. Bevy's StandardMaterial multiplies
                        // base_color × vertex_color × texture.
                        base_color: Color::WHITE,
                        base_color_texture: sub_texture,
                        alpha_mode,
                        perceptual_roughness: 1.0,
                        reflectance: 0.0,
                        cull_mode: None,
                        ..default()
                    })
                })
                .clone();

            let mut child = commands.spawn((
                MmbOverlay,
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat_handle),
                Transform::default(),
                ChildOf(parent),
            ));
            // Static placements (zone-spawn buildings, free `/load_mmb`
            // overlays) participate in camera occlusion. Entity-attached
            // MMBs (NPCs, PCs, pets) deliberately skip the marker — they
            // move every frame and would force a BVH-build storm.
            if is_static_placement {
                child.insert(crate::components::CameraOccluder);
            }
            // Hover-to-inspect debug HUD. Only meaningful for the
            // static zone-spawn case where the operator wants to chase
            // misplaced wall slabs / unplaced MMBs; entity-attached
            // mounts are already targetable via the entity capsule's
            // own Pickable so adding a second one would confuse the
            // click-to-target system.
            // if is_static_placement {
            child.insert(crate::hud::mesh_debug::mesh_debug_bundle(
                crate::hud::mesh_debug::MmbDebugInfo {
                    file_id: req.file_id,
                    chunk_idx: req.chunk_idx,
                    sub_index,
                    asset_name: loaded.asset_name.clone(),
                    variant_name: sub.variant_name.trim().to_string(),
                },
            ));
            // }
        }

        // Per-event spawn confirmation: only emit for manual `/load_mmb`
        // or `/load_mmb_on` invocations (those have entity_id Some, OR
        // identity scale + zero yaw — i.e. the slash-command shape).
        // The auto-load placement-spawn path fires thousands of events
        // per zone and would flood the chat HUD.
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

    // DIAG-zonegeom: remove after fix. Per-file_id summary of the
    // current event burst (zone-in fires hundreds-to-thousands of
    // LoadMmbRequests in one frame; subsequent frames are quiet).
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
        // Even when zero_submesh is empty, surface the counts so we
        // know the system saw events for the target zone.
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

fn push_system_msg(toasts: &mut MessageWriter<crate::snapshot::ToastEvent>, text: String) {
    // Routes to the same `local_toasts` as direct `push_local_toast`
    // calls did before, via the single drain system — keeps the chat
    // chrome unchanged while letting this loader stay parallel-eligible.
    toasts.write(crate::snapshot::ToastEvent::debug(text));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The remap implements lotus's `bc2_alpha / 8.0 * 255` formula
    /// (see `mmb.slang:107-110`). What we verify here are the three
    /// behavioural properties that matter for the renderer — exact
    /// step values are a consequence of the formula, not a separate
    /// spec, so we don't pin them:
    ///
    /// 1. The discard band (raw 0..15) maps to 0 — gives transparent
    ///    pixels at Mask(0.5).
    /// 2. FFXI's "0x80 = 1.0" opaque threshold (raw 128) maps to 255
    ///    — gives fully opaque leaf bodies.
    /// 3. Raw values empirically seen as opaque (e.g. 136 from
    ///    `ron_wf`'s peak) also saturate to 255.
    /// 4. Monotonicity — the remap is non-decreasing in `raw`.
    #[test]
    fn ffxi_alpha_remap_obeys_lotus_spec() {
        // Discard band.
        assert_eq!(ffxi_alpha_remap(0), 0);
        assert_eq!(ffxi_alpha_remap(15), 0);
        // FFXI's "0x80 = 1.0" threshold should saturate.
        assert_eq!(ffxi_alpha_remap(128), 255);
        assert_eq!(ffxi_alpha_remap(136), 255); // empirical leaf peak
        assert_eq!(ffxi_alpha_remap(255), 255);

        // Monotonic: raw_i <= raw_j ⇒ remap(raw_i) <= remap(raw_j).
        let mut prev = 0u8;
        for raw in 0u16..=255 {
            let cur = ffxi_alpha_remap(raw as u8);
            assert!(
                cur >= prev,
                "remap not monotonic at raw={raw}: prev={prev}, cur={cur}"
            );
            prev = cur;
        }

        // Above-threshold band always saturated.
        for raw in 128u16..=255 {
            assert_eq!(
                ffxi_alpha_remap(raw as u8),
                255,
                "raw {raw} should saturate to 255"
            );
        }
    }
}
