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
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::bone::{self, Skeleton};
use ffxi_dat::texture::{decode_texture, DecodedTexture};
use ffxi_dat::vos2::{parse_vos2, Vos2Mesh};
use ffxi_dat::{walk, ChunkKind, DatRoot};

use crate::scene::TrackedEntities;
use crate::snapshot::SceneState;

/// Parent-side actor state for an NPC rendered via Bevy `SkinnedMesh`.
/// One bone-entity is created per skeleton bone; each holds a `Transform`
/// that the tick system mutates every frame from the current MO2
/// keyframe. Bevy walks the bone-entity hierarchy to compose
/// `GlobalTransform`s; the skinning shader reads those + the per-mesh
/// `SkinnedMeshInverseBindposes` to deform vertices on the GPU.
///
/// One `SkinnedActor` per visible NPC. The actor's multiple OS2 chunks
/// (body parts) all share the same `bone_entities` — only one bone tree
/// is built per entity, regardless of how many `LoadVos2Request`s the
/// dispatcher fires for it.
#[derive(Component, Debug)]
pub struct SkinnedActor {
    /// Actor DAT id. Keys the per-frame tick into `BAKED_SKELETONS` +
    /// `IDLE_ANIMS` so we can recompose the bone transforms from the
    /// current MO2 frame.
    pub dat_id: u32,
    /// One Bevy entity per skeleton bone. Length = skeleton.bones.len().
    /// Each bone entity carries a `Transform`; the tick system writes
    /// the animated local transform every frame.
    pub bone_entities: Vec<Entity>,
}

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
    /// Optional explicit skeleton DAT file_id. When `Some(id)`, the
    /// bake uses `baked_skeleton_for_file(id)` instead of the
    /// race → file_id lookup. Used by the NPC actor dispatcher,
    /// where the skeleton lives in the same DAT as the OS2 mesh
    /// (lotus-ffxi `actor.cpp:36` — `ActorSkeletonStatic::getSkeleton(engine, dat_index)`).
    /// PCs leave this `None`; the race-based path remains.
    pub skeleton_file_id: Option<u32>,
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

/// Enumerate every VertexOs2 chunk index in a DAT file. The NPC
/// actor dispatcher uses this to expand one `Standard` look into N
/// per-chunk `LoadVos2Request`s — one per body-part / mesh segment.
/// Returns an empty vec if the DAT is unreadable or contains no OS2
/// chunks (the latter often means the modelid formula picked a DAT
/// that isn't an actor; the dispatcher logs and moves on).
pub fn enumerate_vos2_chunks(file_id: u32) -> Vec<usize> {
    let Ok(root) = DatRoot::from_env_or_default() else {
        return Vec::new();
    };
    let Ok(loc) = root.resolve(file_id) else {
        return Vec::new();
    };
    let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
        return Vec::new();
    };
    walk(&bytes)
        .enumerate()
        .filter_map(|(i, c)| {
            c.ok()
                .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::VertexOs2))
                .map(|_| i)
        })
        .collect()
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
    /// DAT file id this skeleton came from. Used by the animation
    /// path to look up the matching idle MO2 (same file).
    file_id: u32,
    /// `bind_pose_world()` result cached so we don't recompose the
    /// matrix chain on every VOS2 load. Indexed by skeleton bone id.
    world: Vec<[[f32; 4]; 4]>,
    /// Raw skeleton kept for animation: lets us recompose `pose_world`
    /// with per-bone local overrides from MO2 keyframes.
    raw: Option<std::sync::Arc<ffxi_dat::bone::Skeleton>>,
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
    Some(BakedSkeleton {
        file_id,
        world,
        raw: Some(std::sync::Arc::new(skeleton)),
    })
}

/// Walk a DAT file for an `AnimMo2` (kind 0x2B) chunk whose 3-char
/// name matches `wanted` (e.g. `"idl"`). Returns `None` when the DAT
/// has no matching animation. Mirrors lotus-ffxi's
/// `playAnimation("idl")` lookup pattern.
fn load_idle_animation_for_file(file_id: u32) -> Option<ffxi_dat::anim::Mo2Animation> {
    let root = DatRoot::from_env_or_default().ok()?;
    let loc = root.resolve(file_id).ok()?;
    let bytes = fs::read(loc.path_under(root.root())).ok()?;
    for chunk in walk(&bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }
        // Take the first chunk whose name starts with "idl". DATs
        // often ship multiple LOD rigs (24-bone vs 71-bone idle),
        // both named "idl" — the first one in DAT order is what
        // lotus's actor `playAnimation("idl")` would also pick.
        let prefix = &chunk.name[..3];
        if prefix.eq_ignore_ascii_case(b"idl") {
            if let Ok(anim) = ffxi_dat::anim::parse_mo2(chunk.data, &chunk.name) {
                return Some(anim);
            }
        }
    }
    None
}

/// Per-DAT idle-animation cache. Same shape as [`BAKED_SKELETONS`].
static IDLE_ANIMS: OnceLock<
    std::sync::Mutex<std::collections::HashMap<u32, Option<std::sync::Arc<ffxi_dat::anim::Mo2Animation>>>>,
> = OnceLock::new();

fn idle_anim_for_file(file_id: u32) -> Option<std::sync::Arc<ffxi_dat::anim::Mo2Animation>> {
    let map = IDLE_ANIMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&file_id) {
        return entry.clone();
    }
    let loaded = load_idle_animation_for_file(file_id).map(std::sync::Arc::new);
    guard.insert(file_id, loaded.clone());
    loaded
}

/// Sample frame `frame_idx` of an animation: build per-bone local
/// overrides keyed by skeleton bone id. Bones the animation doesn't
/// touch get `None` (the bake falls back to bind-pose local).
fn anim_frame_overrides(
    anim: &ffxi_dat::anim::Mo2Animation,
    frame_idx: usize,
    bone_count: usize,
) -> Vec<Option<ffxi_dat::bone::BoneLocal>> {
    let mut out: Vec<Option<ffxi_dat::bone::BoneLocal>> = vec![None; bone_count];
    for (&bone, frames) in &anim.per_bone {
        let bi = bone as usize;
        if bi >= bone_count {
            continue;
        }
        // Skip the root bone (id 0). MO2 keyframes are absolute (full
        // local transform per frame, per lotus mo2.cppm); for idle
        // animations the root frame is typically identity, which
        // would OVERWRITE the SK2-baked 270°-Y engine-axis rotation
        // and break `unroll_root_rotation` on the CPU bake path /
        // produce sideways rigs on the GPU SkinnedMesh path. The
        // root never moves during idle anyway, so preserving SK2's
        // bind value is both correct and safe.
        if bi == 0 {
            continue;
        }
        let Some(f) = frames.get(frame_idx) else { continue };
        out[bi] = Some(ffxi_dat::bone::BoneLocal {
            rotation: f.rotation,
            translation: f.translation,
            scale: f.scale,
        });
    }
    out
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

/// Bone[0] of every PC skeleton ships with rotation quat
/// `(0, 0.7071, 0, -0.7071)` — a 270° (= −90°) roll around FFXI's
/// Y axis (forward). That rotation propagates down the parent
/// chain into every bone_world matrix, tipping the rendered
/// character sideways. lotus-ffxi gets away with this because
/// their skin compute shader runs after a separate model-space
/// transform applies on the GPU; for our CPU bake we have to undo
/// it explicitly. Mapping (x, y, z) → (z, y, −x) inverts a 90°
/// rotation around +Y, standing the model upright before the
/// FFXI→Bevy axis flip.
fn unroll_root_rotation(v: [f32; 3]) -> [f32; 3] {
    [v[2], v[1], -v[0]]
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
        Some(m) => unroll_root_rotation(bone::mat4_transform_point(*m, local)),
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
        Some(m) => unroll_root_rotation(bone::mat4_transform_dir(*m, local)),
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
    mut inverse_bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
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
    // Bone-entity reuse: an NPC actor fires multiple LoadVos2Requests
    // (one per OS2 chunk), all for the same wire entity. The first
    // request builds the skeleton bone tree; subsequent requests for
    // the same entity re-use it so all body parts deform together.
    let mut bone_trees: std::collections::HashMap<u32, Vec<Entity>> =
        std::collections::HashMap::new();

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
        // Skeleton resolution: explicit override wins (NPC dispatch
        // case where the skeleton ships in the same DAT as the OS2),
        // otherwise race-keyed lookup (PC equipment dispatch).
        let baked_owned = match req.skeleton_file_id {
            Some(id) => baked_skeleton_for_file(id),
            None => baked_skeleton(req.race),
        };
        // NPCs (skeleton_file_id set) go through the GPU SkinnedMesh
        // path so animations tick on the GPU and don't re-upload mesh
        // attributes every frame. PCs stay on the CPU bake path until
        // the SkinnedMesh refactor extends to handle the mirror copy
        // + the multi-DAT slot composition that PC equipment requires.
        if let (Some(_dat_id), Some(baked)) = (req.skeleton_file_id, baked_owned.as_ref()) {
            if let Some(raw) = baked.raw.as_ref() {
                let existing = bone_trees.get(&req.entity_id).cloned();
                let bone_entities = spawn_skinned_actor(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut images,
                    &mut inverse_bindposes,
                    bevy_e,
                    loaded,
                    raw,
                    existing,
                );
                bone_trees.insert(req.entity_id, bone_entities);
                info!(
                    "skinned actor spawn: file_id={} entity_id={} verts={} groups={}",
                    req.file_id,
                    req.entity_id,
                    loaded.mesh.vertices.len(),
                    loaded.mesh.groups.len(),
                );
                continue;
            }
        }
        spawn_vos2_meshes_with_skeleton(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            bevy_e,
            loaded,
            baked_owned.as_ref(),
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

/// GPU-skinned-mesh spawn path for NPC actors. Builds (or reuses) the
/// per-bone Bevy entity tree under `parent`, then spawns one Bevy mesh
/// per polygon group with `JOINT_INDEX` / `JOINT_WEIGHT` attributes and
/// a `SkinnedMesh` component pointing at the bone entities.
///
/// `inverse_bindposes` are set to identity matrices because OS2
/// vertices are stored in **bone-local** space (matches lotus-ffxi's
/// compute-shader convention). With identity inv-bind, Bevy's skinning
/// formula becomes `bone_global_transform * vertex_pos` — exactly what
/// you want for already-bone-local vertices.
///
/// The first call for an entity returns the freshly-spawned bone-entity
/// vector; subsequent calls (for additional body-part chunks) reuse it
/// via the `existing_bone_entities` arg so multi-OS2 actors deform as
/// one rig.
fn spawn_skinned_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    parent: Entity,
    loaded: &LoadedVos2,
    raw: &std::sync::Arc<Skeleton>,
    existing_bone_entities: Option<Vec<Entity>>,
) -> Vec<Entity> {
    use ffxi_dat::bone::PARENT_ROOT;

    let bone_entities = match existing_bone_entities {
        Some(existing) => existing,
        None => {
            // Two-pass spawn: create all bone entities first so the
            // parent ChildOf can reference them by index, then wire up
            // the parent links. Bones declared as PARENT_ROOT (or
            // self-parenting) get parented to the actor's wire entity.
            //
            // Bone[0] gets identity rotation, not SK2's bind rotation.
            // SK2 stores a 270°-Y "engine-axis" roll on the root bone
            // that the CPU path counters via `unroll_root_rotation`
            // after baking. The SkinnedMesh path can't post-process,
            // so we drop the roll at the source and let everything
            // downstream live in upright bone space. The actor's
            // parent Transform then handles the FFXI→Bevy axis flip.
            let mut ents: Vec<Entity> = Vec::with_capacity(raw.bones.len());
            for (i, bone) in raw.bones.iter().enumerate() {
                let q = bone.rot;
                let rotation = if i == 0 {
                    Quat::IDENTITY
                } else {
                    Quat::from_xyzw(q[0], q[1], q[2], q[3])
                };
                let tf = Transform {
                    translation: Vec3::from_array(bone.trans),
                    rotation,
                    scale: Vec3::ONE,
                };
                let id = commands
                    .spawn((tf, GlobalTransform::default(), Visibility::default()))
                    .id();
                ents.push(id);
            }
            for (i, bone) in raw.bones.iter().enumerate() {
                let p = bone.parent as usize;
                let parent_e = if bone.parent == PARENT_ROOT || p == i || p >= ents.len() {
                    parent
                } else {
                    ents[p]
                };
                commands.entity(ents[i]).insert(ChildOf(parent_e));
            }
            // Insert the parent-side `SkinnedActor` once (only on the
            // first chunk's spawn — the existing-vec branch above
            // skips this).
            commands.entity(parent).insert(SkinnedActor {
                dat_id: raw_dat_id_for_skeleton(raw),
                bone_entities: ents.clone(),
            });
            ents
        }
    };

    let inv_bindposes_handle = inverse_bindposes.add(SkinnedMeshInverseBindposes::from(
        vec![Mat4::IDENTITY; raw.bones.len()],
    ));

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

    // Per-vertex joint attributes. OS2 ships up to 2 bone indices per
    // vertex (`Vos2BoneIndices::bone_index1` / `bone_index2`). We
    // populate slots [0]/[1] with these and leave [2]/[3] as zeros
    // (weight 0). Weight is currently 1.0 on bone_index1 — multi-bone
    // weight blending is a follow-up that needs the `Vos2Vertex`
    // weight field to be exposed (only `bone_index1` per vertex
    // surfaces today via `skeleton_bone_for`).
    let n = loaded.mesh.vertices.len();
    let mut joint_indices: Vec<[u16; 4]> = vec![[0u16; 4]; n];
    let mut joint_weights: Vec<[f32; 4]> = vec![[1.0, 0.0, 0.0, 0.0]; n];
    for i in 0..n {
        let bone = loaded.mesh.skeleton_bone_for(i).unwrap_or(0);
        joint_indices[i][0] = bone;
        // Clamp to skeleton bone count — Bevy crashes (or silently
        // skins to zero) on out-of-range joint indices. The
        // `skeleton_fits_mesh` check in the CPU path guards this for
        // bake-path meshes; replicate the guard here.
        if (bone as usize) >= raw.bones.len() {
            joint_indices[i][0] = 0;
            joint_weights[i] = [0.0; 4];
        }
    }

    let positions: Vec<[f32; 3]> = loaded.mesh.vertices.iter().map(|v| v.pos).collect();
    let normals: Vec<[f32; 3]> = loaded.mesh.vertices.iter().map(|v| v.normal).collect();

    for group in &loaded.mesh.groups {
        if group.triangles.is_empty() {
            continue;
        }
        let mut uvs: Vec<[f32; 2]> = vec![[0.0, 0.0]; n];
        let mut uv_set: Vec<bool> = vec![false; n];
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
        let tex_handle = by_name
            .get(&group.texture_name)
            .cloned()
            .or_else(|| {
                let trimmed = group.texture_name.trim_start_matches("tim").trim();
                by_name.get(trimmed).cloned()
            })
            .or_else(|| first.clone());

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        // Vec<[u16; 4]> isn't auto-converted to VertexAttributeValues
        // (no `From` impl); spell out the Uint16x4 variant explicitly.
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            bevy::mesh::VertexAttributeValues::Uint16x4(joint_indices.clone()),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights.clone());
        mesh.insert_indices(Indices::U32(indices));

        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: tex_handle.clone(),
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
            SkinnedMesh {
                inverse_bindposes: inv_bindposes_handle.clone(),
                joints: bone_entities.clone(),
            },
        ));
    }

    bone_entities
}

/// `SkinnedActor.dat_id` recovery: we have `&Arc<Skeleton>` but no
/// back-pointer to the file_id. Look it up via the `BAKED_SKELETONS`
/// cache by scanning entries — there are only a handful per session
/// (one per visible actor model), so a linear scan is fine.
fn raw_dat_id_for_skeleton(raw: &std::sync::Arc<Skeleton>) -> u32 {
    if let Some(map) = BAKED_SKELETONS.get() {
        if let Ok(g) = map.lock() {
            for (k, v) in g.iter() {
                if let Some(b) = v {
                    if let Some(rr) = &b.raw {
                        if std::sync::Arc::ptr_eq(rr, raw) {
                            return *k;
                        }
                    }
                }
            }
        }
    }
    // Falling back to 0 means the tick system can't find the
    // animation for this actor; the NPC will freeze at bind pose.
    // Shouldn't happen for entries that came through the cache.
    0
}

/// Per-frame animation tick. For each `SkinnedActor`, advance the
/// current MO2 frame and write the per-bone local transform onto
/// each `bone_entities[i]`'s `Transform`. Bevy auto-composes
/// `GlobalTransform`s along the hierarchy; the skinning shader then
/// deforms vertices on the GPU.
pub fn tick_skinned_actors(
    time: Res<Time>,
    q_actors: Query<&SkinnedActor>,
    mut q_bones: Query<&mut Transform>,
) {
    let elapsed = time.elapsed_secs();
    for actor in &q_actors {
        let Some(baked) = baked_skeleton_for_file(actor.dat_id) else {
            continue;
        };
        let Some(raw) = baked.raw else { continue };
        let Some(anim) = idle_anim_for_file(actor.dat_id) else {
            continue;
        };
        if anim.frames == 0 {
            continue;
        }
        let safe_speed = if anim.speed > 0.0 { anim.speed } else { 1.0 };
        let frame_idx = ((elapsed / safe_speed).floor() as usize) % anim.frames as usize;

        for (i, bone) in raw.bones.iter().enumerate() {
            let Some(&bone_e) = actor.bone_entities.get(i) else {
                continue;
            };
            // Sample the animated local for this bone if MO2 drives it;
            // fall back to the bone's bind-time local otherwise.
            let (rot, trans, scale) = match anim
                .per_bone
                .get(&(i as u32))
                .and_then(|frames| frames.get(frame_idx))
            {
                Some(f) => (f.rotation, f.translation, f.scale),
                None => (bone.rot, bone.trans, [1.0, 1.0, 1.0]),
            };
            if let Ok(mut tf) = q_bones.get_mut(bone_e) {
                tf.rotation = Quat::from_xyzw(rot[0], rot[1], rot[2], rot[3]);
                tf.translation = Vec3::from_array(trans);
                tf.scale = Vec3::from_array(scale);
            }
        }
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
    let baked = baked_skeleton(race);
    spawn_vos2_meshes_with_skeleton(commands, meshes, materials, images, parent, loaded, baked.as_ref());
}

/// Same as [`spawn_vos2_meshes`] but takes the resolved skeleton
/// directly — used by the NPC actor dispatcher which resolves the
/// skeleton from the actor's own DAT file_id, not from a race byte.
fn spawn_vos2_meshes_with_skeleton(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    loaded: &LoadedVos2,
    baked_owned: Option<&BakedSkeleton>,
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

    // Pose-bake: if the actor's DAT has an idle MO2 animation, sample
    // its first frame and use the resulting per-bone world transforms
    // instead of the bind pose. This puts NPCs in their natural ready
    // stance (arms relaxed, slight stance offset) rather than the
    // bind-time T-pose. PCs without an idle anim in their equipment
    // DAT fall back to bind pose automatically.
    //
    // NPCs are dispatched through `spawn_skinned_actor` (GPU skin)
    // instead — this CPU-bake path now only runs for PCs (equipment
    // DATs typically have no idle MO2, so `posed_owned` ends up
    // `None` and the existing bind-pose fallback applies).
    let posed_owned: Option<BakedSkeleton> = baked_owned.and_then(|b| {
        let raw = b.raw.as_ref()?;
        let anim = idle_anim_for_file(b.file_id)?;
        if anim.frames == 0 {
            return None;
        }
        let overrides = anim_frame_overrides(&anim, 0, raw.bones.len());
        Some(BakedSkeleton {
            file_id: b.file_id,
            world: raw.pose_world(&overrides),
            raw: b.raw.clone(),
        })
    });
    let effective = posed_owned.as_ref().or(baked_owned);
    // `baked_for_mesh` returns None when the skeleton doesn't fit
    // (bone-index out of range), in which case the helpers fall
    // back to local-space rendering — the pre-bake behavior, small
    // and contained at the entity origin.
    let baked = baked_for_mesh(&loaded.mesh, effective);
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

    // Mirror copy: VOS2 stores only one symmetric half of the body;
    // the other half is generated by mirroring across the body's
    // left/right axis. lotus-ffxi encodes per-vertex bone2 +
    // mirror_axis bits to drive this, but for a humanoid the mirror
    // is consistently across the X axis in post-bake model space.
    // We render the mirror as a parallel mesh with X-flipped
    // positions and normals; mirror_axis=0 vertices (spine
    // centerline) z-fight harmlessly with themselves and the user
    // never notices. Only spawn the mirror when the bake actually
    // applied — local-space fallbacks already render at the entity
    // origin, and a mirror copy at the same place is wasted work.
    let mirror_positions: Vec<[f32; 3]> = if baked.is_some() {
        positions.iter().map(|p| [-p[0], p[1], p[2]]).collect()
    } else {
        Vec::new()
    };
    let mirror_normals: Vec<[f32; 3]> = if baked.is_some() {
        normals.iter().map(|n| [-n[0], n[1], n[2]]).collect()
    } else {
        Vec::new()
    };

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

        let tex_handle = by_name
            .get(&group.texture_name)
            .cloned()
            .or_else(|| {
                let trimmed = group.texture_name.trim_start_matches("tim").trim();
                by_name.get(trimmed).cloned()
            })
            .or_else(|| first.clone());

        // Closure: build + spawn one mesh from a (positions, normals)
        // pair. UVs and indices are shared between the primary and
        // mirror copies — only the per-vertex pos/normal differs.
        // Material is per-spawn so the mirror gets its own
        // `Handle<StandardMaterial>` (Bevy doesn't mutably share
        // materials between meshes anyway, so this is the natural
        // shape).
        let spawn_one = |commands: &mut Commands,
                         meshes: &mut Assets<Mesh>,
                         materials: &mut Assets<StandardMaterial>,
                         pos: Vec<[f32; 3]>,
                         norm: Vec<[f32; 3]>,
                         uvs: Vec<[f32; 2]>,
                         idx: Vec<u32>| {
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, norm);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_indices(Indices::U32(idx));

            let mat = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: tex_handle.clone(),
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
        };

        // Primary: just the data we already computed.
        spawn_one(
            commands,
            meshes,
            materials,
            positions.clone(),
            normals.clone(),
            uvs.clone(),
            indices.clone(),
        );

        // Mirror: only spawn when the bake actually applied (a
        // non-baked mesh is at the entity origin and a mirror would
        // just z-fight). Triangle indices are unchanged: the
        // mirrored vertex pool keeps the same indexing, only the
        // per-vertex (pos, normal) has its X component flipped.
        if !mirror_positions.is_empty() {
            spawn_one(
                commands,
                meshes,
                materials,
                mirror_positions.clone(),
                mirror_normals.clone(),
                uvs,
                indices,
            );
        }
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
