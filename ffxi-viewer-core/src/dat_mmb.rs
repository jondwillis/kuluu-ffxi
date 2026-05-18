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
use crate::snapshot::SceneState;

/// Marker for overlay entities spawned by this module — lets the
/// `/load_mmb clear` command (future work) find and despawn them.
#[derive(Component)]
pub struct MmbOverlay;

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
            .init_resource::<crate::dat_mzb::LastAutoLoadedZone>()
            .init_resource::<crate::dat_mzb::DrawDistance>()
            .init_resource::<crate::dat_mzb::MzbCollisionGeometry>()
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
                    crate::dat_mzb::process_load_mzb_requests,
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
                    // GPU-skinned NPC actor tick: writes each bone
                    // entity's Transform from the current MO2 frame.
                    // Bevy's skinning shader reads the resulting
                    // GlobalTransforms and deforms vertices on the
                    // GPU; no per-frame mesh-attribute mutation.
                    crate::dat_vos2::tick_skinned_actors,
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
        let colors: Vec<[f32; 4]> = m
            .vertices
            .iter()
            .map(|v| {
                [
                    v.rgba[0] as f32 / 255.0,
                    v.rgba[1] as f32 / 255.0,
                    v.rgba[2] as f32 / 255.0,
                    v.rgba[3] as f32 / 255.0,
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
        });
    }

    let asset_name = header.asset_name_str().trim().to_string();
    Ok(LoadedMmb {
        submeshes: out,
        textures,
        asset_name,
    })
}

/// Convert a [`DecodedTexture`] into a Bevy [`Image`] asset. The
/// texture decoder produces top-mip RGBA8 already; we just wrap it in
/// the asset type Bevy expects for `base_color_texture`.
fn decoded_texture_to_image(t: &DecodedTexture) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        t.rgba.clone(),
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
    mut scene_state: ResMut<SceneState>,
    tracked: Res<TrackedEntities>,
) {
    // Collect events up front so we can dedupe per-file IO. A zone-in
    // for a city like Bastok Markets fires ~1000+ events that all hit
    // one file_id; parsing each MMB chunk + decoding the IMG pool once
    // (instead of once per event) is the difference between a CPU lock
    // and a one-frame hitch.
    let queued: Vec<LoadMmbRequest> = events.read().copied().collect();
    if queued.is_empty() {
        return;
    }

    // Cache one LoadedMmb per (file_id, chunk_idx).
    let mut mmb_cache: std::collections::HashMap<(u32, usize), Option<LoadedMmb>> =
        std::collections::HashMap::new();
    // Cache image handles per file_id (each IMG chunk in a DAT is
    // shared across all MMBs from that file).
    let mut tex_pools: std::collections::HashMap<
        u32,
        (
            std::collections::HashMap<String, Handle<Image>>,
            Option<Handle<Image>>,
        ),
    > = std::collections::HashMap::new();

    for req in queued {
        let loaded_entry = mmb_cache
            .entry((req.file_id, req.chunk_idx))
            .or_insert_with(|| load_mmb(req.file_id, req.chunk_idx).ok());
        let Some(loaded) = loaded_entry.as_ref() else {
            push_system_msg(
                &mut scene_state,
                format!("/load_mmb {} {}: load failed", req.file_id, req.chunk_idx),
            );
            continue;
        };

        if loaded.submeshes.is_empty() {
            // Suppress for zone-spawn events (req.world_transform is
            // Some). Hundreds of MMBs in a city zone are clod-style
            // sub-records we don't decode yet (task #18); spamming
            // chat for each one drowns out actual operator messages.
            if req.world_transform.is_none() {
                push_system_msg(
                    &mut scene_state,
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
        let pool_is_new = !tex_pools.contains_key(&req.file_id);
        let pool = tex_pools.entry(req.file_id).or_insert_with(|| {
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

        // One-shot per-DAT diagnostic: when the texture pool is freshly
        // built, log the texture-name pool plus the unique submesh
        // texture names this MMB asked for and which ones resolved.
        // Helps diagnose "missing textures" symptoms — set
        // RUST_LOG=ffxi_viewer_core::dat_mmb=info to see it.
        if pool_is_new {
            // `SMMBHeader.pieces` (lotus mmb.cppm:98) sits at the first
            // 4 bytes of the payload — i.e. decrypted bytes 32..36 from
            // the file. We don't yet decode the block headers, so we
            // just probe `pieces` to compare against what our heuristic
            // scanner actually returned. A mismatch (pieces > 0 but
            // we see far fewer than `numModel * pieces` submeshes)
            // tells us the scanner is missing structural records.
            let img_names: Vec<&str> = tex_by_name.keys().map(|s| s.as_str()).collect();
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
            info!(
                target: "ffxi_viewer_core::dat_mmb",
                file_id = req.file_id,
                chunk_idx = req.chunk_idx,
                asset = %loaded.asset_name,
                submesh_count = loaded.submeshes.len(),
                img_count = tex_by_name.len(),
                imgs = ?img_names,
                matched = ?matched,
                unmatched = ?unmatched,
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
                        &mut scene_state,
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
                let mut e = commands.spawn((MmbOverlay, parent_transform, Visibility::default()));
                if is_zone_spawn {
                    e.insert(crate::dat_mzb::AutoMzbOverlay);
                }
                e.id()
            }
        };

        let n_subs = loaded.submeshes.len();
        for (sub_index, sub) in loaded.submeshes.iter().enumerate() {
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, sub.positions.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, sub.normals.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, sub.colors.clone());
            mesh.insert_indices(Indices::U32(sub.indices.clone()));

            let variant_trimmed = sub.variant_name.trim();
            let sub_texture = tex_by_name
                .get(variant_trimmed)
                .cloned()
                .or_else(|| first_texture.clone());

            let mat = materials.add(StandardMaterial {
                // WHITE so the mesh's per-vertex `ATTRIBUTE_COLOR`
                // (FFXI's baked vertex lighting) and the bound
                // `base_color_texture` (if any) both pass through
                // un-tinted. Bevy's StandardMaterial multiplies
                // base_color × vertex_color × texture.
                base_color: Color::WHITE,
                base_color_texture: sub_texture,
                perceptual_roughness: 1.0,
                reflectance: 0.1,
                // UNLIT is load-bearing: FFXI MMBs ship pre-rotated
                // vertex normals and pre-baked vertex colors (the
                // "lighting" is already painted into the mesh data).
                // Letting PBR re-light produces (a) visible specular
                // → "shiny stone walls", (b) sun bleed through wall
                // cracks at dawn/dusk, (c) dark triangular patches
                // on floors where the pre-rotated normals point
                // away from the engine sun. Don't disable without
                // also stripping the baked-color/normal channels.
                //
                // Regression note: commit b31a17e ("sun_moon: fix
                // black-disc bug") collateral-damaged this line —
                // the commit's stated scope was sun/moon disc HDR,
                // but its diff also commented out the MMB unlit.
                // The visible symptom was "missing textures" on
                // floors/walls/stairs in dim zones (Bastok Mines),
                // because PBR re-lighting against FFXI's pre-baked
                // normals leaves whole surfaces nearly black even
                // when the textures are correctly paired.
                unlit: true,
                // FFXI triangle-strip winding isn't pinned to a
                // canonical front/back convention — render both
                // sides instead of guessing.
                cull_mode: None,
                ..default()
            });

            let mut child = commands.spawn((
                MmbOverlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
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
            if is_static_placement {
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
                &mut scene_state,
                format!(
                    "/load_mmb {} {}: spawned {n_subs} sub-mesh{} {where_}{tex_note}",
                    req.file_id,
                    req.chunk_idx,
                    if n_subs == 1 { "" } else { "es" },
                ),
            );
        }
    }
}

fn push_system_msg(scene_state: &mut SceneState, text: String) {
    use ffxi_viewer_wire::{ChatChannel, ChatLine};
    // `push_local_toast`, not `snapshot.chat.push`: the snapshot's chat
    // buffer is server-owned and the next ingest tick overwrites it.
    // `local_toasts` persists across ticks until the cap evicts it.
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
    });
}
