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
    /// Race byte from the entity's `EntityLook::Equipped` block.
    /// Used by the bind-pose bake to pick the matching skeleton
    /// (race → skeleton file_id via lotus-ffxi's PCSkeletonIDs
    /// table). `0` means "no race info" and disables the bake.
    pub race: u8,
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

/// `PCSkeletonIDs` from lotus-ffxi `actor_data.cppm` — the eight PC
/// race slots' skeleton file_ids. Index = `race - 1`.
///
/// | race | name      | file_id |
/// |------|-----------|---------|
/// |  1   | Hume M    |  7072   |
/// |  2   | Hume F    | 10248   |
/// |  3   | Elvaan M  | 13424   |
/// |  4   | Elvaan F  | 16600   |
/// |  5   | Taru M    | 19776   |
/// |  6   | Taru F    | 19776 *(shared with Taru M)* |
/// |  7   | Mithra    | 23176   |
/// |  8   | Galka     | 26352   |
///
/// Monstrosity/beastman races (race > 8, e.g. 29 = Kuu Mohzolhil)
/// are not in this table — lotus-ffxi keeps no race→skeleton table
/// for them, and the right `file_id` lives in LSB's
/// `models.h`/`CMobEntity::look` rather than the client. Those will
/// fall through to "no bake" until that lookup lands.
const PC_SKELETON_FILE_IDS: [u32; 8] = [7072, 10248, 13424, 16600, 19776, 19776, 23176, 26352];

/// Resolve `race` byte to a skeleton file_id, or `None` for an
/// unsupported race (0 = uninitialized; >8 = monstrosity / beastman).
fn skeleton_file_id_for_race(race: u8) -> Option<u32> {
    let idx = race.checked_sub(1)? as usize;
    PC_SKELETON_FILE_IDS.get(idx).copied()
}

/// Per-file skeleton cache. Keyed by `file_id` (not race) because
/// Taru M and Taru F share file 19776 — we'd otherwise parse it
/// twice. Outer `OnceLock` because we initialize the map lazily;
/// inner `Mutex<HashMap>` because `OnceLock::get_or_init` only
/// helps for a *single* value, not an open-ended set.
static BAKED_SKELETONS: OnceLock<std::sync::Mutex<std::collections::HashMap<u32, Option<BakedSkeleton>>>> =
    OnceLock::new();

#[derive(Clone)]
struct BakedSkeleton {
    /// `bind_pose_world()` result cached so we don't recompose the
    /// matrix chain on every VOS2 load. Indexed by skeleton bone id.
    /// The raw `Skeleton` is dropped after composition — bind-pose
    /// is the only thing the bake needs, and animation (which would
    /// require the original local transforms) is out of scope.
    world: Vec<[[f32; 4]; 4]>,
}

/// Load the skeleton for a given DAT `file_id`. Scans the file for
/// the first `0x29` (Bone) chunk — lotus-ffxi does the same
/// (`actor_skeleton_static.cpp` walks `dat.root->children` and
/// dynamic_casts) because the chunk index within a skeleton DAT is
/// not stable across files. Cached forever once resolved.
fn baked_skeleton_for_file(file_id: u32) -> Option<BakedSkeleton> {
    let map = BAKED_SKELETONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&file_id) {
        return entry.clone();
    }
    let loaded = load_skeleton(file_id);
    guard.insert(file_id, loaded.clone());
    loaded
}

fn load_skeleton(file_id: u32) -> Option<BakedSkeleton> {
    let root = DatRoot::from_env_or_default().ok()?;
    let loc = root.resolve(file_id).ok()?;
    let bytes = fs::read(loc.path_under(root.root())).ok()?;
    let chunks = walk(&bytes).filter_map(Result::ok);
    let chunk = chunks
        .into_iter()
        .find(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Bone))?;
    let skeleton = Skeleton::parse(chunk.data).ok()?;
    let world = skeleton.bind_pose_world();
    info!(
        "vos2 bake: loaded skeleton file={} bones={}",
        file_id,
        world.len(),
    );
    Some(BakedSkeleton { world })
}

/// Resolve `race` → BakedSkeleton, or `None` when the race has no
/// known skeleton or the load failed. Cached by file_id so Taru M/F
/// share one entry.
fn baked_skeleton(race: u8) -> Option<BakedSkeleton> {
    let file_id = skeleton_file_id_for_race(race)?;
    baked_skeleton_for_file(file_id)
}

/// Decide whether the loaded skeleton is a plausible match for this
/// mesh. Returns `false` when *any* bone the mesh would reference
/// falls outside the skeleton's bone count — the signature of a
/// race mismatch (e.g., a Tarutaru body whose palette indexes bone
/// 98 against our hardcoded 94-bone hum_ skeleton).
///
/// When the skeleton doesn't fit, we'd rather render the mesh in
/// bone-local space (the pre-bake crumpled blob — small and
/// contained at the entity origin) than do a mixed bake where SOME
/// verts go to wrong bone positions in our skeleton, which
/// produces the giant-spike silhouette seen on race=4 Tarutaru in
/// the first bake screenshots.
fn skeleton_fits_mesh(baked: &BakedSkeleton, mesh: &Vos2Mesh) -> bool {
    let n = baked.world.len();
    if mesh.header.use_bone_table() {
        mesh.bone_table.iter().all(|&b| (b as usize) < n)
    } else {
        mesh.bone_indices
            .iter()
            .all(|bi| (bi.bone_index1 as usize) < n)
    }
}

/// Effective skeleton for a single mesh: `Some(baked)` when the
/// hardcoded skeleton's bone count covers every bone the mesh would
/// reference, `None` otherwise. Computed once per VOS2 spawn so the
/// per-vertex helpers stay branch-light.
fn baked_for_mesh<'a>(
    mesh: &Vos2Mesh,
    baked: Option<&'a BakedSkeleton>,
) -> Option<&'a BakedSkeleton> {
    baked.filter(|b| skeleton_fits_mesh(b, mesh))
}

/// Apply the skeleton's bind-pose `bone_world` matrix to one local
/// vertex position, returning the model-space position. Caller is
/// expected to pass `baked = None` when the skeleton doesn't fit
/// this mesh (race mismatch); the helper then returns `local`
/// untouched, which mirrors the pre-bake behavior.
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
    // Per-batch caches still help: 8 nearby PCs in the same gear
    // fire identical (file_id, chunk_idx) requests within a frame,
    // and we don't want to re-parse the DAT or re-upload textures.
    let mut load_cache: std::collections::HashMap<(u32, usize), Option<LoadedVos2>> =
        std::collections::HashMap::new();
    let mut despawned: std::collections::HashSet<u32> = std::collections::HashSet::new();

    for req in queued {
        let Some(&bevy_e) = tracked.by_id.get(&req.entity_id) else {
            // Wire entity gone (zoned out before the load resolved).
            continue;
        };
        let entry = load_cache
            .entry((req.file_id, req.chunk_idx))
            .or_insert_with(|| load_vos2(req.file_id, req.chunk_idx).ok());
        let Some(loaded) = entry.as_ref() else {
            // Silent on per-equip-slot load failures: an Equipped
            // look fires up to 8 requests, many slots may not have a
            // real file (sentinel ids, beastman race extrapolation,
            // etc.). Per-failure chat toasts drown the HUD.
            continue;
        };
        // `scene_state` is reserved for future per-spawn diagnostic
        // toasts; reference it so the borrow checker is content.
        let _ = &scene_state;
        if loaded.mesh.groups.is_empty() || loaded.mesh.vertices.is_empty() {
            continue;
        }
        // Hide the debug capsule once per entity (subsequent slot
        // requests for the same entity find it already gone).
        if despawned.insert(req.entity_id) {
            commands.entity(bevy_e).remove::<Mesh3d>();
        }
        spawn_vos2_meshes(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            bevy_e,
            loaded,
            req.race,
        );
        info!(
            "vos2 spawn: file_id={} entity_id={} verts={} groups={}",
            req.file_id,
            req.entity_id,
            loaded.mesh.vertices.len(),
            loaded.mesh.groups.len(),
        );
    }
}

/// Spawn one polygon-group's worth of Bevy meshes per group, each
/// parented to `parent`, transforming vertices through the race's
/// bind-pose skeleton along the way. Pure data → Bevy commands; no
/// dependency on wire events, so the launcher preview can call it
/// directly with a hand-built `LoadedVos2`.
///
/// Texture handles are uploaded inline (one per `Vos2NamedTexture`).
/// The caller carries the asset writers because they're per-Bevy-
/// `App` and can't be globals.
pub fn spawn_vos2_meshes(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    loaded: &LoadedVos2,
    race: u8,
) {
    // Build per-file texture pool. Reuses the lotus-ffxi VOS2
    // convention that group names like `"tim     em_b61_3"` use the
    // `tim ` prefix to flag a texture-name slot.
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

    // Bind-pose bake: lift bone-local vertices to model space via
    // `bone_world[bone_id] * local_pos` before the FFXI→Bevy axis
    // flip. `baked_for_mesh` returns None when the skeleton doesn't
    // fit (race mismatch or monstrosity race), in which case the
    // helpers fall back to local-space rendering — the pre-bake
    // behavior, small and contained at the entity origin.
    let baked_owned = baked_skeleton(race);
    let baked = baked_for_mesh(&loaded.mesh, baked_owned.as_ref());
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
        // VertexOs2 stores UVs per-corner; a single vertex may
        // appear with multiple UVs across groups. We approximate by
        // taking each vertex's *first* UV-as-seen — visually
        // imperfect on seams but avoids splitting the vertex buffer.
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

        let tex_handle = by_name
            .get(&group.texture_name)
            .cloned()
            .or_else(|| {
                let trimmed = group.texture_name.trim_start_matches("tim").trim();
                by_name.get(trimmed).cloned()
            })
            .or_else(|| first.clone());

        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: tex_handle,
            perceptual_roughness: 1.0,
            cull_mode: None,
            ..default()
        });

        commands.spawn((
            Vos2Overlay,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(mat),
            Transform::default(),
            ChildOf(parent),
        ));
    }
}

/// Compose: resolve each of the 8 equipment slots to a DAT file via
/// the equipment formula, load each VOS2 chunk, and spawn it under
/// `parent` with the race's bind-pose skeleton applied. Slots set
/// to `0`-id sentinels are silently skipped (no item equipped).
///
/// Returns the number of slots that actually produced geometry —
/// the launcher can use this to decide whether to fall back to a
/// placeholder when the spawn was a total miss.
pub fn spawn_equipped(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    race: u8,
    head: u16,
    body: u16,
    hands: u16,
    legs: u16,
    feet: u16,
    main: u16,
    sub: u16,
    ranged: u16,
) -> usize {
    use crate::look_resolver::resolve_equipment_slot;
    let slots = [head, body, hands, legs, feet, main, sub, ranged];
    let mut spawned = 0usize;
    for slot_id in slots {
        let Some(file_id) = resolve_equipment_slot(slot_id, race) else {
            continue;
        };
        let Ok(loaded) = load_vos2(file_id, 4) else { continue };
        if loaded.mesh.groups.is_empty() || loaded.mesh.vertices.is_empty() {
            continue;
        }
        spawn_vos2_meshes(commands, meshes, materials, images, parent, &loaded, race);
        spawned += 1;
    }
    spawned
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
