//! VertexOs2 (chunk kind 0x2A) Bevy spawn path. Mirrors `dat_mmb.rs`
//! for the skinned-mesh format used by PC bodies and humanoid NPCs.
//!
//! The two pipelines stay parallel rather than unified because the
//! formats themselves are structurally different — MMB is sub-record
//! based with one mesh per record, VertexOs2 is opcode-driven with
//! one mesh and multiple polygon groups bound to texture names. The
//! caller (look_resolver) picks the right pipeline based on what's in
//! the resolved DAT chunk's kind byte.
//!
//! Skinning is intentionally out of scope: vertices render in their
//! bind-pose positions. The plan's non-goals list captures this —
//! proper deform requires a Sk2 (0x29) parser and per-frame bone
//! matrices, which are separate efforts.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::sync::OnceLock;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::bone::{self, Skeleton};
use ffxi_dat::texture::{decode_texture, DecodedTexture};
use ffxi_dat::vos2::{parse_vos2, Vos2Mesh};
use ffxi_dat::{walk, ChunkKind, DatRoot};

use crate::scene::TrackedEntities;
use crate::snapshot::SceneState;

/// Marker component for VertexOs2-spawned meshes — parallel to
/// `MmbOverlay`, used by debug-clear paths to find these specifically.
#[derive(Component)]
pub struct Vos2Overlay;

/// Spawn-a-VertexOs2 request. Look-driven only at the moment;
/// `entity_id` is always `Some` (free-floating overlay spawning was
/// the MMB pipeline's debug affordance and is unnecessary here).
#[derive(Message, Debug, Clone, Copy)]
pub struct LoadVos2Request {
    pub file_id: u32,
    pub chunk_idx: usize,
    pub entity_id: u32,
}

/// One named texture decoded from an IMG chunk colocated with the
/// VertexOs2 in the same DAT file. The name matches what the
/// polygon-block's texture-name opcode set per group.
#[derive(Debug, Clone)]
pub struct Vos2NamedTexture {
    pub name: String,
    pub texture: DecodedTexture,
}

/// One VertexOs2 load: parsed mesh + texture pool.
pub struct LoadedVos2 {
    pub mesh: Vos2Mesh,
    pub textures: Vec<Vos2NamedTexture>,
}

/// Load + parse a VertexOs2 chunk at `(file_id, chunk_idx)`. Errors
/// surface as `Err(String)` so the caller can push a chat-HUD toast.
pub fn load_vos2(file_id: u32, chunk_idx: usize) -> Result<LoadedVos2, String> {
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    // Equipment DATs typically have multiple VertexOs2 chunks at
    // different LODs. `chunk_idx` is the caller's hint, but only used
    // when it actually IS a VertexOs2 chunk. Otherwise fall back to
    // "largest VertexOs2 in the file" — empirically the high-LOD body
    // mesh, which is what we want to render.
    let chunk_at_hint = chunks
        .get(chunk_idx)
        .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::VertexOs2));
    let chunk = match chunk_at_hint {
        Some(c) => c,
        None => chunks
            .iter()
            .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::VertexOs2))
            .max_by_key(|c| c.data.len())
            .ok_or_else(|| format!("no VertexOs2 chunk in file {file_id}"))?,
    };

    let mesh = parse_vos2(chunk.data).map_err(|e| format!("vos2 parse: {e}"))?;

    // Scrape IMG chunks for textures the same way MMB does. Equipment
    // DATs typically have one IMG per body part.
    let textures: Vec<Vos2NamedTexture> = chunks
        .iter()
        .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Img))
        .filter_map(|c| {
            let texture = decode_texture(c.data).ok()?;
            let name = ffxi_dat::texture::extract_texture_name(c.data).unwrap_or_default();
            Some(Vos2NamedTexture { name, texture })
        })
        .collect();

    Ok(LoadedVos2 { mesh, textures })
}

/// Hardcoded humanoid skeleton for the bind-pose bake. File 7072
/// chunk[70] is the 94-bone "hum_" skeleton confirmed in
/// [[sk2-format]]; using it for every race is a known wrong-but-
/// useful first cut — Tarutaru / Galka / Mithra are anatomically
/// different and will distort until we have a real race → skeleton
/// mapping. The distortion is informative: it tells us whether the
/// bake math is correct (recognizable bipedal silhouette = yes) or
/// not (still crumpled = no).
const HARDCODED_SKELETON_FILE_ID: u32 = 7072;
const HARDCODED_SKELETON_CHUNK_IDX: usize = 70;

/// Lazily-loaded skeleton + pre-composed bind-pose world matrices.
/// `None` if the load failed (file missing, parse error, etc.) —
/// callers then fall back to bone-local positions (the pre-bake
/// behavior). One read for the entire session.
static BAKED_SKELETON: OnceLock<Option<BakedSkeleton>> = OnceLock::new();

struct BakedSkeleton {
    /// `bind_pose_world()` result cached so we don't recompose the
    /// matrix chain on every VOS2 load. Indexed by skeleton bone id.
    /// The raw `Skeleton` is dropped after composition — bind-pose
    /// is the only thing the bake needs, and animation (which would
    /// require the original local transforms) is out of scope.
    world: Vec<[[f32; 4]; 4]>,
}

fn baked_skeleton() -> Option<&'static BakedSkeleton> {
    BAKED_SKELETON
        .get_or_init(|| {
            let root = DatRoot::from_env_or_default().ok()?;
            let loc = root.resolve(HARDCODED_SKELETON_FILE_ID).ok()?;
            let bytes = fs::read(loc.path_under(root.root())).ok()?;
            let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
            let chunk = chunks.get(HARDCODED_SKELETON_CHUNK_IDX)?;
            if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Bone) {
                return None;
            }
            let skeleton = Skeleton::parse(chunk.data).ok()?;
            let world = skeleton.bind_pose_world();
            info!(
                "vos2 bake: loaded skeleton file={} chunk={} bones={}",
                HARDCODED_SKELETON_FILE_ID,
                HARDCODED_SKELETON_CHUNK_IDX,
                world.len(),
            );
            Some(BakedSkeleton { world })
        })
        .as_ref()
}

/// Apply the skeleton's bind-pose `bone_world` matrix to one local
/// vertex position, returning the model-space position. Falls back
/// to the local position when no skeleton is loaded or the vertex's
/// resolved bone id is out of range — equivalent to the pre-bake
/// behavior of treating the vertex pool as already in model space.
fn bake_position(
    mesh: &Vos2Mesh,
    vertex_idx: usize,
    local: [f32; 3],
    baked: Option<&BakedSkeleton>,
) -> [f32; 3] {
    let Some(baked) = baked else { return local };
    let Some(bone_id) = mesh.skeleton_bone_for(vertex_idx) else {
        return local;
    };
    match baked.world.get(bone_id as usize) {
        Some(m) => bone::mat4_transform_point(*m, local),
        None => local,
    }
}

/// Same as [`bake_position`] but for normals — rotation-only
/// (translation column discarded by [`bone::mat4_transform_dir`]).
fn bake_normal(
    mesh: &Vos2Mesh,
    vertex_idx: usize,
    local: [f32; 3],
    baked: Option<&BakedSkeleton>,
) -> [f32; 3] {
    let Some(baked) = baked else { return local };
    let Some(bone_id) = mesh.skeleton_bone_for(vertex_idx) else {
        return local;
    };
    match baked.world.get(bone_id as usize) {
        Some(m) => bone::mat4_transform_dir(*m, local),
        None => local,
    }
}

fn decoded_texture_to_image(t: &DecodedTexture) -> Image {
    Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        t.rgba.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// Consume `LoadVos2Request` events and spawn one mesh entity per
/// polygon group under the tracked Bevy entity. The vertex pool is
/// shared across groups (one `Mesh::ATTRIBUTE_POSITION` per group,
/// referencing the same `pos`/`normal` data), since each group's
/// index list points into the same vertex pool.
pub fn process_load_vos2_requests(
    mut events: MessageReader<LoadVos2Request>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut scene_state: ResMut<SceneState>,
    tracked: Res<TrackedEntities>,
) {
    let queued: Vec<LoadVos2Request> = events.read().copied().collect();
    if queued.is_empty() {
        return;
    }
    // Cache loads per (file_id, chunk_idx) — equipping a new helm on
    // 8 nearby PCs of the same race fires 8 identical requests.
    let mut cache: std::collections::HashMap<(u32, usize), Option<LoadedVos2>> =
        std::collections::HashMap::new();
    let mut tex_pools: std::collections::HashMap<
        u32,
        (
            std::collections::HashMap<String, Handle<Image>>,
            Option<Handle<Image>>,
        ),
    > = std::collections::HashMap::new();

    for req in queued {
        let entry = cache
            .entry((req.file_id, req.chunk_idx))
            .or_insert_with(|| load_vos2(req.file_id, req.chunk_idx).ok());
        let Some(loaded) = entry.as_ref() else {
            // Silent on per-equip-slot load failures: an Equipped look
            // fires up to 8 requests, many slots may not have a real
            // file (sentinel ids, beastman race extrapolation, etc.).
            // Per-failure chat toasts drown the HUD with noise.
            continue;
        };
        // Reference the unused `scene_state` to satisfy the borrow
        // checker now that the toast path is gone.
        let _ = &scene_state;
        if loaded.mesh.groups.is_empty() || loaded.mesh.vertices.is_empty() {
            // No renderable geometry — silently skip.
            continue;
        }

        let Some(&bevy_e) = tracked.by_id.get(&req.entity_id) else {
            // Wire entity gone (zoned out before the load resolved).
            continue;
        };
        // Hide the debug capsule — same approach as MMB pipeline.
        commands.entity(bevy_e).remove::<Mesh3d>();

        // Build per-file texture pool once.
        let pool = tex_pools.entry(req.file_id).or_insert_with(|| {
            let mut by_name: std::collections::HashMap<String, Handle<Image>> =
                std::collections::HashMap::with_capacity(loaded.textures.len());
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

        // Bind-pose bake: VOS2 vertices are authored in **bone-local
        // space** (each vertex relative to its assigned skeleton
        // bone), so they must be lifted to model space by
        // `bone_world[bone_id] * local_pos` before the FFXI→Bevy
        // axis flip. Without this step the slot meshes (head, body,
        // legs, etc.) all pile up at the entity origin — the
        // "crumpled" rendering we saw earlier. See [[sk2-format]].
        //
        // Skeleton is hardcoded today (file 7072 chunk[70], hum_)
        // because race→skeleton_file_id mapping is unsolved.
        // Non-humanoid races will distort; that's a known follow-up.
        // When the skeleton load fails (missing DAT, parse error)
        // the helpers fall back to local positions, restoring the
        // pre-bake behavior so the renderer keeps working.
        let baked = baked_skeleton();
        // FFXI vertices are in left-handed (X-right, Y-forward,
        // Z-up). Bevy is right-handed Y-up. Mirror the
        // `scene::ffxi_to_bevy` transform: (x, y, z) → (x, -z, -y)
        // for both positions and normals so lighting stays
        // consistent with the surface orientation.
        let positions: Vec<[f32; 3]> = loaded
            .mesh
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let p = bake_position(&loaded.mesh, i, v.pos, baked);
                [p[0], -p[2], -p[1]]
            })
            .collect();
        let normals: Vec<[f32; 3]> = loaded
            .mesh
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let n = bake_normal(&loaded.mesh, i, v.normal, baked);
                [n[0], -n[2], -n[1]]
            })
            .collect();

        for group in &loaded.mesh.groups {
            if group.triangles.is_empty() {
                continue;
            }
            // VertexOs2 stores UVs per-corner (per-triangle), so a
            // single vertex may appear with multiple UVs across
            // different groups. We approximate by taking each
            // vertex's *first* UV-as-seen and using that for the
            // whole vertex. Visually-imperfect on UV seams, but
            // avoids splitting the vertex buffer — a Phase-N
            // refactor can do proper per-corner expansion if seam
            // artifacts become an issue.
            let mut uvs: Vec<[f32; 2]> = vec![[0.0, 0.0]; loaded.mesh.vertices.len()];
            let mut uv_set: Vec<bool> = vec![false; loaded.mesh.vertices.len()];
            let mut indices: Vec<u32> = Vec::with_capacity(group.triangles.len() * 3);
            for t in &group.triangles {
                for c in 0..3 {
                    let i = t.indices[c] as usize;
                    if i < uvs.len() && !uv_set[i] {
                        uvs[i] = t.uvs[c];
                        uv_set[i] = true;
                    }
                    indices.push(t.indices[c] as u32);
                }
            }

            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_indices(Indices::U32(indices));

            // Texture binding: VertexOs2 group names typically look
            // like `"tim     em_b61_3"` — the leading `"tim     "`
            // is a fixed tag with the asset name following. Try the
            // full name first, then fall back to the first texture.
            let tex_handle = pool
                .0
                .get(&group.texture_name)
                .cloned()
                .or_else(|| {
                    // Drop the `tim     ` prefix and try the rest.
                    let trimmed = group.texture_name.trim_start_matches("tim").trim();
                    pool.0.get(trimmed).cloned()
                })
                .or_else(|| pool.1.clone());

            let mat = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: tex_handle,
                perceptual_roughness: 0.7,
                cull_mode: None,
                ..default()
            });

            commands.spawn((
                Vos2Overlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                Transform::default(),
                ChildOf(bevy_e),
            ));
        }
        info!(
            "vos2 spawn: file_id={} entity_id={} verts={} groups={}",
            req.file_id,
            req.entity_id,
            loaded.mesh.vertices.len(),
            loaded.mesh.groups.len(),
        );

        // No per-request toast spam — Equipped looks fire 8 of these
        // per PC, and a busy zone would drown the chat HUD.
    }
}

fn push_system_msg(scene_state: &mut SceneState, text: String) {
    use ffxi_viewer_wire::{ChatChannel, ChatLine};
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
    });
}
