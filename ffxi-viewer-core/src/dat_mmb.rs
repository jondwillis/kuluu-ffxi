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
use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
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
    let root = DatRoot::from_env_or_default()
        .map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = chunks
        .get(chunk_idx)
        .ok_or_else(|| format!("file has {} chunks, idx {chunk_idx} out of range", chunks.len()))?;
    if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
        return Err(format!(
            "chunk {chunk_idx} kind={:#x} ({:?}), not an MMB",
            chunk.kind,
            ChunkKind::label(chunk.kind),
        ));
    }

    let decrypted = mmb::decrypt(chunk.data).map_err(|e| format!("decrypt: {e}"))?;
    let header = MmbHeader::parse(&decrypted).map_err(|e| format!("header parse: {e}"))?;
    let subs = MmbSubRecord::find_all(header.payload);

    // Stage 5b: scrape IMG chunks from the same DAT. Many files have
    // dozens of IMGs (file 200 = 53; file 133 = 47); a *correct*
    // submesh→texture pairing would route through MMB tex0/test00
    // sub-records, but those aren't decoded yet. The first decodable
    // IMG is what we'll bind to all submeshes for now — adequate for
    // single-material props (Wells, HomePoints, MogHouse furniture).
    let textures: Vec<NamedTexture> = chunks
        .iter()
        .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Img))
        .filter_map(|c| {
            let texture = decode_texture(c.data).ok()?;
            let name = ffxi_dat::texture::extract_texture_name(c.data).unwrap_or_default();
            Some(NamedTexture { name, texture })
        })
        .collect();

    let mut out = Vec::with_capacity(subs.len());
    for sub in &subs {
        // Skip sub-records that aren't the standard `"model   "` tag —
        // clod-style and other tags use a different body layout that
        // `parse_vertices` cannot decode as 36-byte-stride. Without
        // this filter, mis-parsed vertices appear as enormous shards
        // radiating from each placement (Phase 8 / task #18).
        if !sub.tag.starts_with(b"model") {
            continue;
        }
        // Skip sub-records whose body can't fit a 36-byte vertex stride
        // for the declared count, or whose strip yields no triangles
        // after restart/winding decode.
        let Some(verts) = sub.parse_vertices() else { continue };
        let tris = sub.parse_triangle_list();
        if tris.is_empty() {
            continue;
        }
        let positions: Vec<[f32; 3]> = verts.iter().map(|v| v.pos).collect();
        let normals: Vec<[f32; 3]> = verts.iter().map(|v| v.normal).collect();
        let uvs: Vec<[f32; 2]> = verts.iter().map(|v| v.uv).collect();
        let colors: Vec<[f32; 4]> = verts
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
        // Drop triangles whose indices reference past the actual
        // vertex array. The strip-length header at the head of the
        // index buffer and stray strip-restart sentinels can encode
        // u16 values well above the vertex count; rendering them
        // samples adjacent-buffer garbage and produces enormous
        // shards that stretch out from the placement point. Bounds-
        // check here keeps the visible geometry tight.
        let vert_count = verts.len() as u16;
        let indices: Vec<u32> = tris
            .iter()
            .filter(|t| t[0] < vert_count && t[1] < vert_count && t[2] < vert_count)
            .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();
        out.push(MmbSubMesh {
            variant_name: sub.variant_name_str(),
            positions,
            normals,
            uvs,
            colors,
            indices,
        });
    }

    Ok(LoadedMmb {
        submeshes: out,
        textures,
    })
}

/// Convert a [`DecodedTexture`] into a Bevy [`Image`] asset. The
/// texture decoder produces top-mip RGBA8 already; we just wrap it in
/// the asset type Bevy expects for `base_color_texture`.
fn decoded_texture_to_image(t: &DecodedTexture) -> Image {
    Image::new(
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
    )
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
        (std::collections::HashMap<String, Handle<Image>>, Option<Handle<Image>>),
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

        // Two parenting modes:
        //
        // - `entity_id = Some(id)` (look-driven or `/load_mmb_on`): hang
        //   the meshes under the existing `WorldEntity` so they inherit
        //   its world transform, and strip the entity's debug capsule
        //   `Mesh3d` so the real model replaces the placeholder.
        // - `entity_id = None` (free overlay, original `/load_mmb`):
        //   spawn a new parent at `world_pos`.
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
        for sub in loaded.submeshes.iter() {
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
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
                perceptual_roughness: 0.7,
                // FFXI triangle-strip winding isn't yet pinned to a
                // canonical front/back convention — render both sides
                // until Phase 8.5 settles winding (then flip back to
                // `Some(Face::Back)` for proper culling/lighting).
                cull_mode: None,
                ..default()
            });

            commands.spawn((
                MmbOverlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                Transform::default(),
                ChildOf(parent),
            ));
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
