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
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::bone::{self, Skeleton};
use ffxi_dat::texture::{decode_texture, DecodedTexture};
use ffxi_dat::vos2::{parse_vos2, Vos2Mesh};
use ffxi_dat::{walk, walk_tree, ChunkKind, ChunkNode, DatRoot};

use crate::scene::TrackedEntities;

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
    /// The pivot entity sitting between `parent` and `bone_entities[0]`,
    /// carrying the FFXI-engine→Bevy axis flip (Q_x(π) for PCs,
    /// IDENTITY for NPCs) and the feet-at-origin translation
    /// (`Vec3::Y * -actor_min_local_y`). Subsequent slot loads update
    /// its translation when a deeper min surfaces — all slots of one
    /// actor must share the same translation, or per-slot shifts will
    /// disassemble the rig.
    pub pivot: Entity,
    /// Running minimum local-Y observed across every slot loaded for
    /// this actor so far. Used to decide whether a new slot's deeper
    /// min requires updating the pivot translation.
    pub min_local_y: f32,
    /// Running maximum local-Y (head extent across slots). Surfaces in
    /// `BakedActor.actor_height` for nameplate / camera anchors.
    pub max_local_y: f32,
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
    // Tree-scoped: only top-level OS2 children of the outermost Rmp
    // container. Mirrors lotus's `dat.root->children` iteration —
    // nested Rmps' OS2 children (LODs, mirror copies) are NOT
    // included. The flat walker over-collects and produces
    // duplicate-geometry artifacts.
    let tree = walk_tree(&bytes);
    let container = top_container(&tree);
    container
        .children
        .iter()
        .enumerate()
        .filter(|(_, n)| ChunkKind::from_u8(n.chunk.kind) == Some(ChunkKind::VertexOs2))
        .map(|(i, _)| i)
        .collect()
}

/// Return the conceptual "root container" of a DAT — the first Rmp
/// child of the synthetic root, or the synthetic root itself if the
/// file is flat (no Rmp wrapper). lotus calls this `dat.root` and
/// iterates `root->children` for top-level content.
fn top_container<'r, 'a>(tree: &'r ChunkNode<'a>) -> &'r ChunkNode<'a> {
    tree.children.first().unwrap_or(tree)
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

    // Tree-scoped chunk lookup. Per lotus's actor loader
    // (`actor_skeleton.cpp:89` iterates `dat.root->children`), only
    // direct children of the outermost Rmp container are part of
    // "this asset" — chunks nested inside child Rmps are LOD/mirror
    // variants the renderer skips. Our previous flat walk
    // over-collected those and overlaid duplicate geometry on the
    // baseline mesh (visible as "tube arms" / doubled meshes).
    let tree = walk_tree(&bytes);
    let container = top_container(&tree);
    let os2_children: Vec<&ChunkNode<'_>> = container
        .children
        .iter()
        .filter(|n| ChunkKind::from_u8(n.chunk.kind) == Some(ChunkKind::VertexOs2))
        .collect();

    // `chunk_idx` is now an index into the filtered OS2-children
    // list (matching `enumerate_vos2_chunks`). When the caller's hint
    // is out of range, fall back to "largest OS2 child" — empirically
    // the high-LOD mesh for that asset.
    let node = match os2_children.get(chunk_idx) {
        Some(n) => *n,
        None => os2_children
            .iter()
            .copied()
            .max_by_key(|n| n.chunk.data.len())
            .ok_or_else(|| format!("no VertexOs2 child of root Rmp in file {file_id}"))?,
    };
    let mesh = parse_vos2(node.chunk.data).map_err(|e| format!("vos2 parse: {e}"))?;

    // Diagnostic for empty meshes — dump the top container's
    // immediate children with their kinds so we can spot files
    // whose geometry actually lives in a non-OS2 chunk type.
    if mesh.groups.is_empty() || mesh.vertices.is_empty() {
        let kinds: Vec<String> = container
            .children
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let k = ChunkKind::from_u8(n.chunk.kind)
                    .map(|x| format!("{:?}", x))
                    .unwrap_or_else(|| format!("0x{:02x}", n.chunk.kind));
                let name = std::str::from_utf8(&n.chunk.name).unwrap_or("?");
                format!("[{}]{}({},{}B)", i, k, name.trim(), n.chunk.data.len())
            })
            .collect();
        info!(
            "vos2 empty-mesh dump file={} top_children: {}",
            file_id,
            kinds.join(" ")
        );
    }

    // Textures: scoped to the same top container. Both legacy `Img`
    // and `Dxt3` chunks are surfaced — lotus's actor loader handles
    // both, and equipment DATs often use Dxt3.
    let textures: Vec<Vos2NamedTexture> = container
        .children
        .iter()
        .filter(|n| ChunkKind::from_u8(n.chunk.kind) == Some(ChunkKind::Img))
        .filter_map(|n| {
            let texture = decode_texture(n.chunk.data).ok()?;
            let name = ffxi_dat::texture::extract_texture_name(n.chunk.data).unwrap_or_default();
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
pub fn skeleton_file_id_for_race(race: u8) -> Option<u32> {
    let idx = race.checked_sub(1)? as usize;
    PC_SKELETON_FILE_IDS.get(idx).copied()
}

/// Per-file skeleton cache. Keyed by `file_id` (not race) because
/// Taru M and Taru F share file 19776 — we'd otherwise parse it
/// twice. Outer `OnceLock` because we initialize the map lazily;
/// inner `Mutex<HashMap>` because `OnceLock::get_or_init` only
/// helps for a *single* value, not an open-ended set.
static BAKED_SKELETONS: OnceLock<
    std::sync::Mutex<std::collections::HashMap<u32, Option<BakedSkeleton>>>,
> = OnceLock::new();

#[derive(Clone)]
struct BakedSkeleton {
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
    let map =
        BAKED_SKELETONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
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
    // Per-race bake-extent diagnostic. The PC skeleton bind pose
    // determines character height; logging min/max y across all
    // bones surfaces any race with anomalous proportions (Taru
    // should be shorter, Galka taller). Bake-y here is in FFXI bake
    // space (pre-axis-flip); post-flip y range = pre-flip z range.
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY);
    for bone in &world {
        // World matrix is row-major (per `ffxi-dat/src/bone.rs:235`):
        // translation lives at `m[0][3], m[1][3], m[2][3]`.
        let x = bone[0][3];
        let y = bone[1][3];
        let z = bone[2][3];
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
        if z < min_z {
            min_z = z;
        }
        if z > max_z {
            max_z = z;
        }
    }
    info!(
        "vos2 bake: loaded skeleton file={} bones={} \
         bake_x=[{:.2}..{:.2}] dx={:.2} \
         bake_y=[{:.2}..{:.2}] dy={:.2} \
         bake_z=[{:.2}..{:.2}] dz={:.2}",
        file_id,
        world.len(),
        min_x,
        max_x,
        max_x - min_x,
        min_y,
        max_y,
        max_y - min_y,
        min_z,
        max_z,
        max_z - min_z,
    );
    Some(BakedSkeleton {
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
    std::sync::Mutex<
        std::collections::HashMap<u32, Option<std::sync::Arc<ffxi_dat::anim::Mo2Animation>>>,
    >,
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
        // bone_table is the palette; per-vertex bone_index reads from it.
        // If the palette fits, every weight1 and weight2 reference fits.
        mesh.bone_table.iter().all(|&b| (b as usize) < n)
    } else {
        // Direct indices — `bone_indices` interleaves bone1/bone2 records
        // (2 entries per FFXI vertex per lotus's reader). Both must fit
        // or the bake will silently fall back to local-space for some
        // verts, scattering the resulting mesh.
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
///
/// 2-weight vertices: when the vertex has an entry in
/// `mesh.bone_weights` (i.e. its index falls in the `weight2`
/// region), this blends the two bones' contributions per the FFXI
/// formula `w1*bone1*pos1 + w2*bone2*pos2`. For rigid (`weight1`)
/// verts the path collapses to the single-bone case.
///
/// Why this matters: races whose body meshes carry many 2-weight
/// verts (Mithra has tail bones, Galka has sash bones) baked
/// with the old single-bone approximation produced vertices spread
/// 1+ yalms along the forward axis, making the body appear missing
/// from typical camera angles.
fn bake_position(
    mesh: &Vos2Mesh,
    vertex_idx: usize,
    _local: [f32; 3],
    baked: Option<&BakedSkeleton>,
) -> [f32; 3] {
    let Some(baked) = baked else { return _local };
    let weight1_count = mesh.vertices.len().saturating_sub(mesh.bone_weights.len());
    if vertex_idx >= weight1_count && vertex_idx < mesh.vertices.len() {
        let bw = &mesh.bone_weights[vertex_idx - weight1_count];
        let b1 = mesh.skeleton_bone_for(vertex_idx).map(|b| b as usize);
        let b2 = mesh.skeleton_bone2_for(vertex_idx).map(|b| b as usize);
        let m1 = b1.and_then(|i| baked.world.get(i));
        let m2 = b2.and_then(|i| baked.world.get(i));
        // Only do the 2-bone blend if *both* bones resolve. With one
        // bone unresolved, the fallback (untransformed pos) is in a
        // different frame than the other bone's world-transformed
        // result; mixing them produces vertices in nonsense
        // locations — visible as the "mouth blown apart" / "body
        // stretched 1y forward" failures we saw on Mithra and
        // Galka. Single-bone bake on bone1 is a safer degradation.
        if let (Some(m1), Some(m2)) = (m1, m2) {
            // Lotus two-weight blend (animation_skin.slang:56,63):
            //   pos = (R1*p1) + t1*w1 + (R2*p2) + t2*w2
            // Rotations apply to FULL positions; only translations are
            // weight-blended. Our previous code did
            // `w1*(R1*p1+t1) + w2*(R2*p2+t2)`, which under-weighted the
            // rotated position by (1-w) and produced joint-region
            // distortion (long necks, wide hips, skinny shoulders).
            let sum = bw.weight1 + bw.weight2;
            let (w1, w2) = if sum > 0.0 {
                (bw.weight1 / sum, bw.weight2 / sum)
            } else {
                (1.0, 0.0)
            };
            let r1p1 = bone::mat4_transform_dir(*m1, bw.pos1);
            let r2p2 = bone::mat4_transform_dir(*m2, bw.pos2);
            let t1 = [m1[0][3], m1[1][3], m1[2][3]];
            let t2 = [m2[0][3], m2[1][3], m2[2][3]];
            let blended = [
                r1p1[0] + r2p2[0] + t1[0] * w1 + t2[0] * w2,
                r1p1[1] + r2p2[1] + t1[1] * w1 + t2[1] * w2,
                r1p1[2] + r2p2[2] + t1[2] * w1 + t2[2] * w2,
            ];
            return unroll_root_rotation(blended);
        }
        // Fallback: rigid single-bone on bone1 using pos1.
        if let Some(m1) = m1 {
            return unroll_root_rotation(bone::mat4_transform_point(*m1, bw.pos1));
        }
        return bw.pos1;
    }
    // Rigid (1-weight) vertex: single-bone transform of mesh.vertices[i].pos.
    let local = mesh
        .vertices
        .get(vertex_idx)
        .map(|v| v.pos)
        .unwrap_or(_local);
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
/// 2-weight verts blend normals from both bones per the same w1/w2
/// weighting; result is *not* renormalized (matches lotus's CPU
/// path; the shader doesn't care about exact unit length for
/// directional lighting at typical model scales).
fn bake_normal(
    mesh: &Vos2Mesh,
    vertex_idx: usize,
    _local: [f32; 3],
    baked: Option<&BakedSkeleton>,
) -> [f32; 3] {
    let Some(baked) = baked else { return _local };
    let weight1_count = mesh.vertices.len().saturating_sub(mesh.bone_weights.len());
    if vertex_idx >= weight1_count && vertex_idx < mesh.vertices.len() {
        let bw = &mesh.bone_weights[vertex_idx - weight1_count];
        let b1 = mesh.skeleton_bone_for(vertex_idx).map(|b| b as usize);
        let b2 = mesh.skeleton_bone2_for(vertex_idx).map(|b| b as usize);
        let m1 = b1.and_then(|i| baked.world.get(i));
        let m2 = b2.and_then(|i| baked.world.get(i));
        if let (Some(m1), Some(m2)) = (m1, m2) {
            // Lotus normal blend (animation_skin.slang:57,64): unweighted
            // sum of rotated normals. The lighting path tolerates unit-
            // length drift; matches our `bake_normal` doc-comment.
            let n1 = bone::mat4_transform_dir(*m1, bw.normal1);
            let n2 = bone::mat4_transform_dir(*m2, bw.normal2);
            let blended = [n1[0] + n2[0], n1[1] + n2[1], n1[2] + n2[2]];
            return unroll_root_rotation(blended);
        }
        if let Some(m1) = m1 {
            return unroll_root_rotation(bone::mat4_transform_dir(*m1, bw.normal1));
        }
        return bw.normal1;
    }
    let local = mesh
        .vertices
        .get(vertex_idx)
        .map(|v| v.normal)
        .unwrap_or(_local);
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
    tracked: Res<TrackedEntities>,
    // Cross-tick state recovery: when a request batch spans multiple
    // frames the in-tick `actor_state` map is empty on the second
    // frame and we read the actor's prior (bones, pivot, min, max)
    // from this component. Within a single tick the in-tick map is
    // the source of truth (Commands::insert is deferred).
    q_skinned_actor: Query<&SkinnedActor>,
    // Cross-tick merge of the CPU-bake actor extent: PC equipment
    // slots arrive across multiple frames, so the running (min, max)
    // already committed for this entity is read back from its
    // `BakedActor` when the in-tick `cpu_extent` map is empty.
    q_baked: Query<&crate::scene::BakedActor>,
    // Pivot re-anchoring: every request whose merged min is deeper
    // than the pivot's current translation rewrites it via this
    // query so the actor stays glued to feet-on-ground.
    mut q_xform: Query<&mut Transform>,
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
    // In-tick actor tracker: bones + pivot + running (min, max)
    // local-y across every slot loaded this tick for each entity.
    // `Commands::insert(SkinnedActor)` is deferred (won't be visible
    // to queries until the command buffer applies after the system
    // finishes), so same-frame multi-slot batches can't see each
    // other through the component — they need this in-tick view.
    //
    // For cross-tick requests (rare, but possible if the look pipeline
    // re-fires across frames), we fall back to the `SkinnedActor`
    // component for state recovery; see the lookup below.
    let mut actor_state: std::collections::HashMap<u32, (Vec<Entity>, Entity, f32, f32)> =
        std::collections::HashMap::new();
    // In-tick running (min, max) parent-local Y for the CPU-bake (PC)
    // path, accumulated across an actor's equipment slots so the
    // `BakedActor` we attach reflects the whole body, not just the
    // last slot processed this tick.
    let mut cpu_extent: std::collections::HashMap<u32, (f32, f32)> =
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
        // GPU SkinnedMesh path: animates on the GPU and doesn't
        // re-upload mesh attributes every frame.
        //
        // NPC dispatch sets `race: 0`; PC dispatch sets `race: <1..=8>`
        // and a race-keyed skeleton file_id. We always take the GPU
        // path when the race skeleton is available — per-vertex
        // fallback in `spawn_skinned_actor` rigidly binds any
        // out-of-range bone to bone[0] (the hip) so individual
        // verts that miss don't vanish. The slot-level fit check
        // is now informational only: it surfaces "how bad is the
        // mismatch" in logs without dropping the whole slot to a
        // CPU bake (which had its own problems — bind-pose meshes
        // overlaid on the GPU body at the wrong origin).
        if let (Some(_dat_id), Some(baked)) = (req.skeleton_file_id, baked_owned.as_ref()) {
            if let Some(raw) = baked.raw.as_ref() {
                let fits = skeleton_fits_mesh(baked, &loaded.mesh);
                let is_pc = req.race != 0;
                {
                    let skel_n = baked.world.len();
                    let max_table =
                        loaded.mesh.bone_table.iter().copied().max().unwrap_or(0) as usize;
                    let max_bone1 = loaded
                        .mesh
                        .bone_indices
                        .iter()
                        .step_by(2)
                        .map(|bi| bi.bone_index1 as usize)
                        .max()
                        .unwrap_or(0);
                    info!(
                        "vos2 dispatch: file_id={} entity_id={} race={} is_pc={} \
                         skel_bones={} use_bone_table={} bone_table_max={} max_bone1={} \
                         fits={} path=GPU",
                        req.file_id,
                        req.entity_id,
                        req.race,
                        is_pc,
                        skel_n,
                        loaded.mesh.header.use_bone_table(),
                        max_table,
                        max_bone1,
                        fits,
                    );
                }
                // Measure THIS slot's local-y extent. Each LoadVos2
                // request brings one OS2 chunk (≈ one body part); a
                // multi-part actor (PC equipment slots, or multi-DAT
                // NPCs) needs the **actor-wide** min to anchor the
                // pivot. Otherwise legs/head chunks each shift by
                // their own slot's min and disassemble the rig.
                //
                // `compute_skinned_local_y_extent` reads `v.pos`
                // directly, which is bone-local — useless for finding
                // where the vertex actually lands. For PCs we go
                // through `measure_post_bake_y_extent`, which applies
                // the bone matrices to get a skeleton-world position
                // and returns `-p[1]` (= post-pivot-rotation Y under
                // the `Q_y(π/2) * Q_x(π)` pivot we install above).
                // NPC pivots are identity and their rigs are Y-up at
                // bone-local, so the raw `v.pos[1]` path still works
                // for them.
                let (slot_min, slot_max) = if is_pc {
                    measure_post_bake_y_extent(loaded, baked_owned.as_ref()).unwrap_or((0.0, 1.9))
                } else {
                    compute_skinned_local_y_extent(loaded, is_pc).unwrap_or((-0.9, 1.6))
                };

                // Resolve prior actor state: in-tick first (visible
                // immediately), then cross-tick component (visible
                // after Commands apply).
                let existing_actor = actor_state
                    .get(&req.entity_id)
                    .map(|(b, p, _, _)| (b.clone(), *p))
                    .or_else(|| {
                        q_skinned_actor
                            .get(bevy_e)
                            .ok()
                            .map(|a| (a.bone_entities.clone(), a.pivot))
                    });
                let (current_min, current_max) = actor_state
                    .get(&req.entity_id)
                    .map(|&(_, _, mn, mx)| (mn, mx))
                    .or_else(|| {
                        q_skinned_actor
                            .get(bevy_e)
                            .ok()
                            .map(|a| (a.min_local_y, a.max_local_y))
                    })
                    .unwrap_or((f32::INFINITY, f32::NEG_INFINITY));

                let (bone_entities, pivot) = spawn_skinned_actor(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut images,
                    &mut inverse_bindposes,
                    bevy_e,
                    loaded,
                    raw,
                    existing_actor,
                    is_pc,
                    slot_min,
                );

                let actor_min_y = current_min.min(slot_min);
                let actor_max_y = current_max.max(slot_max);

                // Re-anchor the pivot every request so it always
                // matches the deepest min seen so far. This is the
                // multi-slot fix: a slot loaded later in the tick
                // (legs after head) can pull the whole actor up by
                // updating the pivot, keeping every slot's geometry
                // aligned at `parent.y + 0 = feet`.
                let piv_y_before = q_xform
                    .get(pivot)
                    .map(|t| t.translation.y)
                    .unwrap_or(f32::NAN);
                if let Ok(mut piv) = q_xform.get_mut(pivot) {
                    piv.translation.y = -actor_min_y;
                }
                let piv_y_after = q_xform
                    .get(pivot)
                    .map(|t| t.translation.y)
                    .unwrap_or(f32::NAN);
                info!(
                    "skin accumulate: ent={} file={} is_pc={} slot=[{:+.3}..{:+.3}] \
                     current=[{:+.3}..{:+.3}] actor=[{:+.3}..{:+.3}] \
                     piv.y {:+.3}->{:+.3}",
                    req.entity_id,
                    req.file_id,
                    is_pc,
                    slot_min,
                    slot_max,
                    current_min,
                    current_max,
                    actor_min_y,
                    actor_max_y,
                    piv_y_before,
                    piv_y_after,
                );

                actor_state.insert(
                    req.entity_id,
                    (bone_entities.clone(), pivot, actor_min_y, actor_max_y),
                );

                let actor_height = (actor_max_y - actor_min_y).max(0.1);
                // `try_insert`: `bevy_e` is from `TrackedEntities`, but the
                // actor can despawn between the async DAT load and this
                // flush. Tolerate that race rather than panic.
                commands.entity(bevy_e).try_insert(SkinnedActor {
                    dat_id: raw_dat_id_for_skeleton(raw),
                    bone_entities,
                    pivot,
                    min_local_y: actor_min_y,
                    max_local_y: actor_max_y,
                });
                commands.entity(bevy_e).try_insert(crate::scene::BakedActor {
                    min_mesh_y: actor_min_y,
                    actor_height,
                });
                info!(
                    "skinned actor spawn: file_id={} entity_id={} verts={} groups={} \
                     slot=[{:.2}..{:.2}] actor=[{:.2}..{:.2}] actor_height={:.2}",
                    req.file_id,
                    req.entity_id,
                    loaded.mesh.vertices.len(),
                    loaded.mesh.groups.len(),
                    slot_min,
                    slot_max,
                    actor_min_y,
                    actor_max_y,
                    actor_height,
                );
                continue;
            }
        }
        // CPU bake path. This is the *primary* PC route — the
        // look_resolver sets `skeleton_file_id: None` for PCs so they
        // skip the GPU path entirely (see look_resolver.rs:715, and
        // memory note `pc_gpu_skinning_blockers`). It's also the
        // degraded path for NPCs whose skeleton doesn't fit.
        //
        // `feet_translation_y` must NOT be `-slot_min`. Each
        // equipment slot's vertices already arrive in a frame where
        // mesh-Z=0 is the actor's foot sole and mesh-Z=+1.9 is the
        // head top (the `bind_to_bevy` rotation in
        // `spawn_vos2_meshes_with_skeleton` puts mesh-Z onto Bevy-Y).
        // Pre-subtracting the slot's *own* min shifts each part's
        // bottom to Y=0 — i.e., head, body, hands, legs, feet all
        // land stacked at the floor (the "pile of overlapping body
        // parts rooted on the ground" symptom).
        //
        // The right value is 0: every slot sits at its natural
        // post-`bind_to_bevy` Y, so body lands at Y=0.93..1.72,
        // head at Y=1.94..2.29, feet at Y=0..0.60. The actor stands
        // upright by construction. Edge case (heels extending below
        // mesh-Z=0): a future per-actor min accumulation could lift
        // by `-min` to keep heels above parent.y, but typical PC
        // skeletons have feet sole exactly at mesh-Z=0, so 0 is
        // correct for nearly all visible cases.
        let fallback_translation = 0.0;
        let slot_extent = spawn_vos2_meshes_with_skeleton(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            bevy_e,
            loaded,
            baked_owned.as_ref(),
            fallback_translation,
        );
        info!(
            "vos2 spawn: file_id={} entity_id={} verts={} groups={}",
            req.file_id,
            req.entity_id,
            loaded.mesh.vertices.len(),
            loaded.mesh.groups.len(),
        );

        // Attach/refresh `BakedActor` from the just-spawned slot's
        // extent. This is the *primary* in-game PC route, yet it used
        // to drop the returned `(min, max)` on the floor — leaving
        // every PC `baked=false`, so `nameplate_anchor_y` fell back to
        // `FALLBACK_ACTOR_HEIGHT` (3.0) and floated the name ~1 yalm
        // above a real ~2.3-yalm crown. The GPU path (above) and the
        // launcher's `spawn_equipped` already do this; the CPU path
        // was the gap. PC-only (`race != 0`): NPCs anchor through the
        // GPU path and only reach here on a rare skeleton misfit, where
        // this frame's extent convention wouldn't match theirs.
        if req.race != 0 {
            if let Some((slot_min, slot_max)) = slot_extent {
                // Merge across the actor's slots: in-tick map first
                // (deferred inserts aren't query-visible yet), then the
                // cross-tick `BakedActor`, then this slot alone.
                let (prev_min, prev_max) = cpu_extent
                    .get(&req.entity_id)
                    .copied()
                    .or_else(|| {
                        q_baked
                            .get(bevy_e)
                            .ok()
                            .map(|b| (b.min_mesh_y, b.min_mesh_y + b.actor_height))
                    })
                    .unwrap_or((f32::INFINITY, f32::NEG_INFINITY));
                let merged_min = prev_min.min(slot_min);
                let merged_max = prev_max.max(slot_max);
                cpu_extent.insert(req.entity_id, (merged_min, merged_max));
                // `try_insert`: actor may despawn between async load and flush.
                commands.entity(bevy_e).try_insert(crate::scene::BakedActor {
                    min_mesh_y: merged_min,
                    actor_height: (merged_max - merged_min).max(0.1),
                });
            }
        }
    }
}

/// Lowest Y a baked-actor vertex reaches in the **parent entity's
/// local frame** for the GPU-skinned spawn path. PCs route their bone
/// tree through a `Q_x(π)` pivot (see `spawn_skinned_actor`), which
/// negates the FFXI-native Y — so parent-local-y = `-v.pos[1]` and the
/// minimum is `-max(v.pos[1])`. NPCs skip the pivot, leaving
/// parent-local-y = `v.pos[1]` and the minimum at `min(v.pos[1])`.
///
/// Measure the post-bake vertical extent of a VOS2 slot **without**
/// spawning anything. Used by `spawn_equipped` to aggregate
/// `min_local_y` across every slot before deciding the actor's
/// feet-at-origin translation — every slot of one actor must share
/// the same translation, otherwise legs/head/body slots shift
/// independently and the assembled character collapses (each slot's
/// `min_local_y` is the lowest vertex of *that slot only*, not the
/// actor as a whole).
///
/// Mirrors the position-bake walk in
/// [`spawn_vos2_meshes_with_skeleton`]: parent-local Y of a baked
/// vertex equals `positions[i][2]` = `-bake_position(...)[1]` (the
/// bind_to_bevy rotation maps mesh-space `(a, b, c)` to `(-b, c, -a)`,
/// so parent-y is `c` = `-p[1]`).
fn measure_post_bake_y_extent(
    loaded: &LoadedVos2,
    baked_owned: Option<&BakedSkeleton>,
) -> Option<(f32, f32)> {
    if loaded.mesh.vertices.is_empty() {
        return None;
    }
    let baked = baked_for_mesh(&loaded.mesh, baked_owned);
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (i, v) in loaded.mesh.vertices.iter().enumerate() {
        let p = bake_position(&loaded.mesh, i, v.pos, baked);
        let local_y = -p[1];
        if local_y < min_y {
            min_y = local_y;
        }
        if local_y > max_y {
            max_y = local_y;
        }
    }
    if min_y.is_finite() && max_y.is_finite() {
        Some((min_y, max_y))
    } else {
        None
    }
}

/// Returns `(min_local_y, max_local_y)` — the vertical extent of all
/// baked vertices in the **pivot's local frame** (i.e., after the
/// pivot's rotation but before its feet-at-origin translation). The
/// difference `max - min` is the actor's visual height in yalms.
///
/// For PCs, the pivot applies `bind_to_bevy = Q_y(π/2) * Q_x(-π/2)`,
/// which maps mesh-local `(a, b, c)` to pivot-local `(-b, c, -a)` —
/// so pivot-local Y of a vertex is `v.pos[2]` (mesh-Z). For NPCs the
/// pivot is identity, so pivot-local Y is just `v.pos[1]`.
///
/// `None` when the mesh carries no vertices (defensive).
fn compute_skinned_local_y_extent(loaded: &LoadedVos2, is_pc: bool) -> Option<(f32, f32)> {
    if loaded.mesh.vertices.is_empty() {
        return None;
    }
    let mut min_local_y = f32::INFINITY;
    let mut max_local_y = f32::NEG_INFINITY;
    for v in &loaded.mesh.vertices {
        let local_y = if is_pc { v.pos[2] } else { v.pos[1] };
        if local_y < min_local_y {
            min_local_y = local_y;
        }
        if local_y > max_local_y {
            max_local_y = local_y;
        }
    }
    if min_local_y.is_finite() && max_local_y.is_finite() {
        Some((min_local_y, max_local_y))
    } else {
        None
    }
}

/// Offline probe: load `(skel_file_id)` + `(mesh_file_id, chunk_idx)`,
/// replicate `spawn_skinned_actor`'s bone-tree composition (bone[0] forced
/// to identity rotation, all other bones from raw SK2 data, parent-chain
/// composed), then walk every vertex through `bone_world * v.pos` and
/// print min/max world-position extents. Compares against
/// `bake_position` (the CPU bake path) for the same vertices so a
/// divergence between the two reveals whether the bug lives in the bone
/// tree, the per-vertex transform, or downstream Bevy plumbing.
///
/// Public so `bin/ffxi-skin-probe.rs` can call it. Does no Bevy work —
/// just `glam` math + `println!`.
pub fn probe_skinned_actor(skel_file_id: u32, mesh_file_id: u32, chunk_idx: usize) {
    let Some(baked) = baked_skeleton_for_file(skel_file_id) else {
        println!("ERR: failed to load skeleton file_id={skel_file_id}");
        return;
    };
    let Some(raw) = baked.raw.as_ref() else {
        println!("ERR: skeleton file_id={skel_file_id} has no raw bone chunk");
        return;
    };
    let loaded = match load_vos2(mesh_file_id, chunk_idx) {
        Ok(l) => l,
        Err(e) => {
            println!("ERR: load_vos2({mesh_file_id},{chunk_idx}): {e}");
            return;
        }
    };

    let n_bones = raw.bones.len();
    println!("== ffxi-skin-probe ==");
    println!("skeleton file_id={skel_file_id} bones={n_bones}");
    println!(
        "mesh     file_id={mesh_file_id} chunk={chunk_idx} verts={} groups={}",
        loaded.mesh.vertices.len(),
        loaded.mesh.groups.len()
    );

    let local_t: Vec<Vec3> = raw
        .bones
        .iter()
        .map(|b| Vec3::from_array(b.trans))
        .collect();
    let local_r_raw: Vec<Quat> = raw
        .bones
        .iter()
        .map(|b| Quat::from_xyzw(b.rot[0], b.rot[1], b.rot[2], b.rot[3]))
        .collect();
    // NEW chain: bone[0] rotation overridden to identity, matching
    // spawn_skinned_actor.
    let mut local_r_new = local_r_raw.clone();
    if !local_r_new.is_empty() {
        local_r_new[0] = Quat::IDENTITY;
    }

    let compose = |local_r: &[Quat]| -> (Vec<Vec3>, Vec<Quat>) {
        let mut wt = vec![Vec3::ZERO; n_bones];
        let mut wr = vec![Quat::IDENTITY; n_bones];
        if n_bones == 0 {
            return (wt, wr);
        }
        wt[0] = local_t[0];
        wr[0] = local_r[0];
        for i in 1..n_bones {
            let p = raw.bones[i].parent as usize;
            if p < n_bones && p != i {
                wt[i] = wt[p] + wr[p] * local_t[i];
                wr[i] = wr[p] * local_r[i];
            } else {
                wt[i] = local_t[i];
                wr[i] = local_r[i];
            }
        }
        (wt, wr)
    };

    let (new_wt, new_wr) = compose(&local_r_new);
    let (old_wt, _old_wr) = compose(&local_r_raw);

    let axis_extent = |v: &[Vec3], axis: usize| -> (f32, f32) {
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for p in v {
            let a = p[axis];
            if a < mn {
                mn = a;
            }
            if a > mx {
                mx = a;
            }
        }
        (mn, mx)
    };

    let (nx0, nx1) = axis_extent(&new_wt, 0);
    let (ny0, ny1) = axis_extent(&new_wt, 1);
    let (nz0, nz1) = axis_extent(&new_wt, 2);
    let (ox0, ox1) = axis_extent(&old_wt, 0);
    let (oy0, oy1) = axis_extent(&old_wt, 1);
    let (oz0, oz1) = axis_extent(&old_wt, 2);
    println!();
    println!("[BONE WORLD POSITIONS]");
    println!(
        "  OLD chain (bone[0]=raw rot): x=[{ox0:+.3}..{ox1:+.3}] y=[{oy0:+.3}..{oy1:+.3}] z=[{oz0:+.3}..{oz1:+.3}]"
    );
    println!(
        "  NEW chain (bone[0]=identity): x=[{nx0:+.3}..{nx1:+.3}] y=[{ny0:+.3}..{ny1:+.3}] z=[{nz0:+.3}..{nz1:+.3}]"
    );
    println!("  OLD matches load_skeleton's bake_y log; NEW is what spawn_skinned_actor renders.");

    // Per-vertex pass: NEW chain bone_world * v.pos, vs bake_position.
    let mut new_verts: Vec<Vec3> = Vec::with_capacity(loaded.mesh.vertices.len());
    let mut cpu_verts: Vec<Vec3> = Vec::with_capacity(loaded.mesh.vertices.len());
    let mut clipped = 0usize;
    for (i, v) in loaded.mesh.vertices.iter().enumerate() {
        let bone1 = loaded.mesh.skeleton_bone_for(i).unwrap_or(0) as usize;
        let bi = if bone1 < n_bones {
            bone1
        } else {
            clipped += 1;
            0
        };
        let v_pos = Vec3::from_array(v.pos);
        new_verts.push(new_wt[bi] + new_wr[bi] * v_pos);
        let p = bake_position(&loaded.mesh, i, v.pos, Some(&baked));
        cpu_verts.push(Vec3::from_array(p));
    }

    let (nvx0, nvx1) = axis_extent(&new_verts, 0);
    let (nvy0, nvy1) = axis_extent(&new_verts, 1);
    let (nvz0, nvz1) = axis_extent(&new_verts, 2);
    let (cvx0, cvx1) = axis_extent(&cpu_verts, 0);
    let (cvy0, cvy1) = axis_extent(&cpu_verts, 1);
    let (cvz0, cvz1) = axis_extent(&cpu_verts, 2);
    println!();
    println!("[VERTEX WORLD POSITIONS]");
    println!(
        "  NEW (bone_world * v.pos):        x=[{nvx0:+.3}..{nvx1:+.3}] y=[{nvy0:+.3}..{nvy1:+.3}] z=[{nvz0:+.3}..{nvz1:+.3}]"
    );
    println!(
        "  CPU bake (bake_position output): x=[{cvx0:+.3}..{cvx1:+.3}] y=[{cvy0:+.3}..{cvy1:+.3}] z=[{cvz0:+.3}..{cvz1:+.3}]"
    );
    println!(
        "  Clipped to bone[0] (out-of-range): {clipped}/{}",
        new_verts.len()
    );

    // Sample first 5 vertices for inspection.
    println!();
    println!("[FIRST 5 VERTICES]");
    println!("  idx bone v.pos                    NEW (runtime)              CPU bake");
    for i in 0..loaded.mesh.vertices.len().min(5) {
        let v = &loaded.mesh.vertices[i];
        let bone1 = loaded.mesh.skeleton_bone_for(i).unwrap_or(0);
        let nv = new_verts[i];
        let cv = cpu_verts[i];
        println!(
            "  {i:>3} {bone1:>4} ({:+.3},{:+.3},{:+.3})  ({:+.3},{:+.3},{:+.3})  ({:+.3},{:+.3},{:+.3})",
            v.pos[0], v.pos[1], v.pos[2], nv.x, nv.y, nv.z, cv.x, cv.y, cv.z
        );
    }

    // ---- Simulate the *full* pipeline the GPU would see ----
    //
    // Bevy's `SkinnedMesh` formula:
    //   vertex_world = joints[i].compute_matrix() * inv_bindposes[i] * v.pos
    //
    // Our `joints[i]` are bone entities under the pivot (child of
    // bevy_e). With `inv_bindposes = IDENTITY` and bevy_e at the
    // world origin, joints[i].compute_matrix() expands to:
    //   pivot.transform * bone[i].chain_in_pivot
    //
    // So the rendered vertex is:
    //   pivot.rotation * (bone_chain * v.pos) + pivot.translation
    //
    // That's what we simulate below for two candidate pivot rotations.

    let actor_min_y = {
        let mut mn = f32::INFINITY;
        // Match what `measure_post_bake_y_extent` returns: -p[1] of CPU bake.
        for p in &cpu_verts {
            let y = -p.y;
            if y < mn {
                mn = y;
            }
        }
        if mn.is_finite() {
            mn
        } else {
            0.0
        }
    };
    let pivot_translation = Vec3::new(0.0, -actor_min_y, 0.0);

    let candidates: &[(&str, Quat)] = &[
        (
            "Q_y(π/2) * Q_x(π) [current]",
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
                * Quat::from_rotation_x(std::f32::consts::PI),
        ),
        (
            "Q_x(π) [original]",
            Quat::from_rotation_x(std::f32::consts::PI),
        ),
        (
            "Q_y(π/2) * Q_x(-π/2) [first attempt]",
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
                * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        ),
        ("IDENTITY", Quat::IDENTITY),
    ];
    println!();
    println!(
        "[POST-PIVOT WORLD POSITIONS]  pivot.translation = (0, -actor_min_y, 0) = (0, {:+.3}, 0)",
        pivot_translation.y
    );
    println!("  candidate                          x-extent         y-extent         z-extent");
    for (name, rot) in candidates {
        let xform = |p: Vec3| -> Vec3 { *rot * p + pivot_translation };
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for p in &new_verts {
            let w = xform(*p);
            mn = mn.min(w);
            mx = mx.max(w);
        }
        println!(
            "  {:35}  [{:+.3}..{:+.3}]  [{:+.3}..{:+.3}]  [{:+.3}..{:+.3}]",
            name, mn.x, mx.x, mn.y, mx.y, mn.z, mx.z
        );
    }

    // ---- Sanity: is v.pos bone-local or mesh-space? ----
    //
    // If bone-local: |v.pos| ≪ |bone_world_t|, because v.pos is the
    // offset *from* the bone's bind pose.
    // If mesh-space: |v.pos| ≈ |bone_world_t * v.pos|, because the
    // vertex's pre-skinned position is already in the skeleton's
    // root frame.
    let mut vpos_mag = 0f32;
    let mut bone_mag = 0f32;
    let mut bake_mag = 0f32;
    for (i, v) in loaded.mesh.vertices.iter().enumerate() {
        let bone1 = loaded.mesh.skeleton_bone_for(i).unwrap_or(0) as usize;
        let bi = bone1.min(n_bones - 1);
        let vp = Vec3::from_array(v.pos);
        vpos_mag = vpos_mag.max(vp.length());
        bone_mag = bone_mag.max(new_wt[bi].length());
        bake_mag = bake_mag.max(cpu_verts[i].length());
    }
    println!();
    println!("[FRAME OF v.pos]");
    println!(
        "  max |v.pos|          = {:.3}  (small = bone-local, big = mesh-space)",
        vpos_mag
    );
    println!("  max |bone_world_t|   = {:.3}", bone_mag);
    println!("  max |baked vertex|   = {:.3}", bake_mag);
    if vpos_mag < bake_mag * 0.5 {
        println!("  → v.pos appears bone-local. identity inv_bindposes is CORRECT.");
    } else {
        println!("  → v.pos appears mesh-space. identity inv_bindposes DOUBLES the transform.");
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
    // `(bones, pivot)` from a prior slot of the same actor, or None
    // for the first slot. The caller (`process_load_vos2_requests`)
    // tracks this in an in-tick HashMap so same-frame slots share
    // bones + pivot before any `Commands::insert(SkinnedActor)`
    // materializes.
    existing_actor: Option<(Vec<Entity>, Entity)>,
    is_pc: bool,
    // Initial pivot translation magnitude for the *first* slot. Set
    // to `-slot_min` so the lowest baked vertex of that slot lands
    // at parent.y = 0. Subsequent slots may push the pivot deeper
    // (caller's responsibility).
    min_local_y: f32,
) -> (Vec<Entity>, Entity) {
    use ffxi_dat::bone::PARENT_ROOT;

    let (bone_entities, pivot) = match existing_actor {
        Some((bones, pivot)) => (bones, pivot),
        None => {
            // Two-pass spawn: create all bone entities first so the
            // parent ChildOf can reference them by index, then wire up
            // the parent links.
            //
            // **Always** insert a pivot entity between `parent` and the
            // bone tree. The pivot carries two responsibilities:
            //
            //   1. Axis convention from FFXI skeleton-world to Bevy.
            //      PC skeletons are authored Y-down (head bones at
            //      negative Y, feet at Y=0; see `bake_y=[-1.90..0]`
            //      in the load_skeleton diagnostic). The CPU bake's
            //      effective rotation on skeleton-world positions is
            //      `Q_y(π/2) * Q_x(-π/2) * Q_x(-π/2) = Q_y(π/2) *
            //      Q_x(π)` (one Q_x(-π/2) is the pre-swap `[p[0],
            //      p[2], -p[1]]` in the positions array; the other
            //      lives in the entity's `bind_to_bevy` rotation).
            //      The GPU path operates directly on skeleton-world
            //      so the pivot needs to carry the *composed* rotation
            //      — anything less leaves the character tipped on its
            //      side and the height axis projecting through the
            //      width/depth extent. NPC rigs land Y-up already, so
            //      they keep identity.
            //   2. Feet-at-origin translation: `Vec3::Y * -min_local_y`
            //      pushes the lowest baked vertex onto parent.y = 0,
            //      which is the invariant the snap and target-ring
            //      assume (transform.y *is* feet-on-ground).
            //
            // Composing in the pivot keeps both responsibilities out of
            // bone[0] — the animation tick would otherwise have to
            // special-case writing back the flip and translation every
            // frame.
            let pivot_rotation = if is_pc {
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
                    * Quat::from_rotation_x(std::f32::consts::PI)
            } else {
                Quat::IDENTITY
            };
            let pivot_translation = Vec3::Y * -min_local_y;
            let root_parent = commands
                .spawn((
                    Transform {
                        translation: pivot_translation,
                        rotation: pivot_rotation,
                        scale: Vec3::ONE,
                    },
                    GlobalTransform::default(),
                    Visibility::default(),
                    ChildOf(parent),
                ))
                .id();
            // Bone[0] gets identity rotation (drops SK2's 270°-Y
            // engine-axis roll). For PCs the pivot carries the
            // bind_to_bevy flip; for NPCs the identity-on-root
            // matches the current rig.
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
                    root_parent
                } else {
                    ents[p]
                };
                commands.entity(ents[i]).insert(ChildOf(parent_e));
            }
            // SkinnedActor insertion happens in the caller — it's
            // the only place that knows the running actor-wide (min,
            // max) across slots. spawn_skinned_actor just returns
            // (bones, pivot) so the caller can wire them up.
            (ents, root_parent)
        }
    };

    let inv_bindposes_handle = inverse_bindposes.add(SkinnedMeshInverseBindposes::from(vec![
            Mat4::IDENTITY;
            raw.bones.len()
        ]));

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
    // vertex via the 2-weight (`weight2`) record format. We populate
    // slot [0] with the primary bone (rigid + skinned share this) and
    // slot [1] with the secondary bone for skinned verts. Weights
    // come from `bone_weights[v - weight1]` for skinned verts; rigid
    // verts get weight = 1.0 on slot [0].
    //
    // Caveat: FFXI's 2-weight format stores **separate positions** for
    // each bone (`pos1` vs `pos2` in `Vos2BoneWeight`). Bevy's
    // `SkinnedMesh` expects one shared position blended by N bones —
    // so we feed `pos1` (already in `vertices[i].pos`) and accept the
    // approximation. The error is invisible for typical body meshes
    // where pos1/pos2 differ only at the joint crease by < 1 mm.
    let n = loaded.mesh.vertices.len();
    let weight2_count = loaded.mesh.bone_weights.len();
    let weight1_count = n.saturating_sub(weight2_count);
    let mut joint_indices: Vec<[u16; 4]> = vec![[0u16; 4]; n];
    let mut joint_weights: Vec<[f32; 4]> = vec![[1.0, 0.0, 0.0, 0.0]; n];
    let mut out_of_range_count = 0usize;
    for i in 0..n {
        let bone1 = loaded.mesh.skeleton_bone_for(i).unwrap_or(0);
        if (bone1 as usize) >= raw.bones.len() {
            // Permissive fallback: rigidly bind to bone[0] (the hip)
            // so the vertex still translates with the actor instead
            // of disappearing to weight=0. Looks rigid (won't follow
            // the limb it was meant for) but visible-and-static beats
            // invisible — and for slots with only a few stray bones
            // out of range (most slots), the rest of the mesh still
            // skins normally.
            joint_indices[i][0] = 0;
            joint_weights[i] = [1.0, 0.0, 0.0, 0.0];
            out_of_range_count += 1;
            continue;
        }
        joint_indices[i][0] = bone1;
        // 2-weight verts: read bone2 + weight pair, populate slot [1].
        if i >= weight1_count {
            let k = i - weight1_count;
            let bw = &loaded.mesh.bone_weights[k];
            let bone2 = loaded.mesh.skeleton_bone2_for(i).unwrap_or(0);
            let bone2_valid = (bone2 as usize) < raw.bones.len();
            let (w1, w2) = if bone2_valid {
                joint_indices[i][1] = bone2;
                (bw.weight1, bw.weight2)
            } else {
                // Secondary bone out of range — degrade to rigid on bone1.
                (1.0, 0.0)
            };
            let sum = w1 + w2;
            if sum > 0.0 {
                joint_weights[i] = [w1 / sum, w2 / sum, 0.0, 0.0];
            }
        }
    }
    if out_of_range_count > 0 {
        info!(
            "vos2 skin: bone_table_max overflowed race skeleton on {}/{} verts (rigidly bound to hip)",
            out_of_range_count, n,
        );
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

        // FFXI specular params from the OS2 DrawState opcode (0x8010)
        // → Bevy PBR. The two formats don't line up 1-to-1:
        //   - `specular_intensity` is roughly a 0..1 'how shiny' knob;
        //     map it onto Bevy's `metallic` so the highlight reads.
        //   - `specular_exponent` is the Phong exponent (higher = tighter
        //     highlight); invert and clamp into Bevy's `perceptual_roughness`
        //     (lower = sharper highlight). Default (exp=0) → matte (1.0).
        // Both are heuristics — lotus stores the raw values without
        // translating to a PBR pipeline, so there's no reference
        // mapping to be exact against.
        // let (roughness, metallic) = pbr_from_specular(
        //     group.specular_exponent,
        //     group.specular_intensity,
        // );
        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: tex_handle.clone(),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            // FFXI character textures are baked diffuse; the engine
            // never authored a Fresnel/spec response. Bevy's default
            // reflectance of 0.5 (4% F0 non-metal) produces visible
            // rim/sheen on cloth and skin. Zero it out — matches the
            // MMB path which makes the same choice.
            reflectance: 0.0,
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

    (bone_entities, pivot)
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
///
/// Animation choice (per-entity, every frame):
///
/// |              | speed = 0          | speed > 0                           |
/// |--------------|--------------------|-------------------------------------|
/// | **engaged**  | `btl` (motion DAT) | `run1` combat-run (motion DAT)      |
/// | **idle**     | `idl` (skel DAT)   | `run0` casual run (skel DAT)        |
///
/// Each step in the chain falls through to the next on a missing
/// asset, so an NPC skeleton without a motion DAT still gets `run` /
/// `idl` from its skel DAT rather than freezing in bind pose.
///
/// Hard toggle, no crossfade yet — a one-frame snap is acceptable
/// on a 30 fps MMO animation model. Crossfade is a future polish
/// layer.
pub fn tick_skinned_actors(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    motion: Res<crate::combat_stance::EntityMotion>,
    rest: Res<crate::combat_stance::RestStance>,
    mut blends: ResMut<crate::combat_stance::AnimationBlends>,
    clip_override: Option<Res<crate::combat_stance::ModelViewerClipOverride>>,
    q_actors: Query<(&crate::components::WorldEntity, &SkinnedActor)>,
    mut q_bones: Query<&mut Transform>,
) {
    use crate::combat_stance::{ClipId, EntityMotion};
    let elapsed = time.elapsed_secs();
    let dt = time.delta_secs();

    // Model-viewer override: when the resource is present, force every
    // actor onto a single named clip — same per-bone keyframe write as
    // the matrix-driven path, just with the engagement / motion / rest
    // selection bypassed. Resolves once (3-char prefix) against the
    // skeleton DAT first, then the PC motion DAT.
    if let Some(over) = clip_override.as_deref() {
        let prefix = override_prefix(&over.clip_name);
        for (_world, actor) in &q_actors {
            let Some(baked) = baked_skeleton_for_file(actor.dat_id) else {
                continue;
            };
            let Some(raw) = baked.raw else { continue };
            let Some(anim) = crate::combat_stance::override_anim_for_skel(actor.dat_id, &prefix)
            else {
                continue;
            };
            if anim.frames == 0 {
                continue;
            }
            let safe_speed = if anim.speed > 0.0 { anim.speed } else { 1.0 };
            let frame_idx = ((elapsed / safe_speed).floor() as usize) % anim.frames as usize;
            for (i, bone) in raw.bones.iter().enumerate() {
                if i == 0 {
                    continue;
                }
                let Some(&bone_e) = actor.bone_entities.get(i) else {
                    continue;
                };
                let (rot, trans, scale) = match anim
                    .frames_for_bone(i)
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
        return;
    }
    // Build a once-per-frame index of (id → bt_target_id) so the inner
    // engagement lookup is O(1). Without this, the per-actor loop did
    // a linear `find()` over the whole snapshot — quadratic in nearby
    // entity count, which is the hot path in crowded zones.
    let bt_target_by_id: std::collections::HashMap<u32, u32> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.bt_target_id))
        .collect();
    for (world, actor) in &q_actors {
        let Some(baked) = baked_skeleton_for_file(actor.dat_id) else {
            continue;
        };
        let Some(raw) = baked.raw else { continue };

        // Engagement comes from the snapshot's `bt_target_id` (the
        // server's auto-attack target — authoritative). Motion is
        // derived from per-frame Bevy Transform deltas by
        // `track_entity_motion_system`. The wire `speed` field is
        // movement *capability* (40 = base run, 0 = bound/stunned),
        // NOT current motion — see [`EntityMotion`] docs.
        let engaged = bt_target_by_id
            .get(&world.id)
            .map(|&t| t != 0)
            .unwrap_or(false);
        let sample = motion.sample(world.id).unwrap_or_default();
        let moving = sample.speed > EntityMotion::MOVE_THRESHOLD;

        // Rest stance (self only): when `/sit` / `/heal` / `/kneel` is
        // active, self plays the sit / hea MO2 uninterruptibly until
        // the [`RestStance`] resource clears (cleared by movement-key
        // press in `dispatch_movement_system`, by re-pressing the
        // bound `Action::Sit` / `Action::Heal`, or by server-driven
        // heal-off when actual translation is detected). No
        // crossfade — same hard toggle as the rest of the matrix.
        let is_self = state
            .snapshot
            .self_char_id
            .map(|sid| sid == world.id)
            .unwrap_or(false);
        if is_self {
            use crate::combat_stance::RestKind;
            let rest_anim = match rest.kind {
                RestKind::Sit => crate::combat_stance::sit_anim_for_skel(actor.dat_id)
                    .or_else(|| idle_anim_for_file(actor.dat_id)),
                RestKind::Heal => crate::combat_stance::heal_anim_for_skel(actor.dat_id)
                    .or_else(|| crate::combat_stance::sit_anim_for_skel(actor.dat_id))
                    .or_else(|| idle_anim_for_file(actor.dat_id)),
                RestKind::None => None,
            };
            if let Some(anim) = rest_anim {
                if anim.frames > 0 {
                    let safe_speed = if anim.speed > 0.0 { anim.speed } else { 1.0 };
                    let frame_idx =
                        ((elapsed / safe_speed).floor() as usize) % anim.frames as usize;
                    for (i, bone) in raw.bones.iter().enumerate() {
                        if i == 0 {
                            continue;
                        }
                        let Some(&bone_e) = actor.bone_entities.get(i) else {
                            continue;
                        };
                        let (rot, trans, scale) = match anim
                            .frames_for_bone(i)
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
                    continue;
                }
            }
        }

        // Pick a logical ClipId. The directional rule (strafe /
        // backpedal / turn-in-place) only applies to the casual,
        // non-engaged locomotion path; the engaged path keeps the
        // simpler (engaged, moving) matrix because retail combat
        // stance has no strafe variants — combat run with a yawed
        // root looks the same in every horizontal direction.
        //
        // Magnitude threshold for the strafe-vs-forward decision is
        // half the run threshold — both components must be non-trivial
        // *and* the strafe component must dominate.
        let dir_threshold = EntityMotion::MOVE_THRESHOLD * 0.5;
        let clip_id = if engaged {
            if moving {
                ClipId::CombatRun
            } else {
                ClipId::BattleIdle
            }
        } else if moving {
            let fwd = sample.forward_component;
            let strafe = sample.strafe_component;
            if strafe.abs() > fwd.abs()
                && strafe.abs() > dir_threshold
                && fwd.abs() > dir_threshold * 0.5
            {
                if strafe > 0.0 {
                    ClipId::StrafeRight
                } else {
                    ClipId::StrafeLeft
                }
            } else if fwd < -dir_threshold {
                ClipId::Backpedal
            } else {
                ClipId::Run
            }
        } else if sample.heading_rate.abs() > EntityMotion::TURN_THRESHOLD_RAD_PER_SEC {
            ClipId::TurnInPlace
        } else {
            ClipId::Idle
        };

        // Resolve a ClipId to (anim, time_scale_signum). Each step is
        // a graceful degradation — NPC skels lack motion DATs, no PC
        // skel ships a `bck`/`stl`/`str`/`trn` clip (probed empty), so
        // the chain always ends on something universally present.
        //
        // INLINE NOTE: empirically the following 3-char prefixes are
        // ABSENT from every retail PC skeleton DAT and motion DAT we
        // could probe:
        //   - `bck` (backpedal)  — fall back to `run` at -1× time scale
        //   - `stl` / `str` (strafe L/R) — fall back to `run` (no sign flip)
        //   - `trn` (turn-in-place)      — fall back to `idl`
        // Walk (`wlk`) IS present on most PC skel DATs. The probe is
        // still in `directional_anim_for_skel` so that beastman / NPC
        // skels carrying these clips (if any) light up automatically.
        let resolve =
            |clip: ClipId| -> Option<(std::sync::Arc<ffxi_dat::anim::Mo2Animation>, f32)> {
                match clip {
                    ClipId::CombatRun => {
                        crate::combat_stance::combat_run_anim_for_skel(actor.dat_id)
                            .or_else(|| crate::combat_stance::run_anim_for_skel(actor.dat_id))
                            .or_else(|| {
                                crate::combat_stance::battle_idle_anim_for_skel(actor.dat_id)
                            })
                            .or_else(|| idle_anim_for_file(actor.dat_id))
                            .map(|a| (a, 1.0))
                    }
                    ClipId::BattleIdle => {
                        crate::combat_stance::battle_idle_anim_for_skel(actor.dat_id)
                            .or_else(|| idle_anim_for_file(actor.dat_id))
                            .map(|a| (a, 1.0))
                    }
                    ClipId::Run => crate::combat_stance::run_anim_for_skel(actor.dat_id)
                        .or_else(|| idle_anim_for_file(actor.dat_id))
                        .map(|a| (a, 1.0)),
                    ClipId::Backpedal => {
                        crate::combat_stance::directional_anim_for_skel(actor.dat_id, b"bck")
                            .map(|a| (a, 1.0))
                            .or_else(|| {
                                // No dedicated bck clip on PC skels — play run
                                // reversed in time to fake the backpedal cycle.
                                crate::combat_stance::run_anim_for_skel(actor.dat_id)
                                    .map(|a| (a, -1.0))
                            })
                            .or_else(|| idle_anim_for_file(actor.dat_id).map(|a| (a, 1.0)))
                    }
                    ClipId::StrafeLeft => {
                        crate::combat_stance::directional_anim_for_skel(actor.dat_id, b"stl")
                            .or_else(|| crate::combat_stance::run_anim_for_skel(actor.dat_id))
                            .or_else(|| idle_anim_for_file(actor.dat_id))
                            .map(|a| (a, 1.0))
                    }
                    ClipId::StrafeRight => {
                        crate::combat_stance::directional_anim_for_skel(actor.dat_id, b"str")
                            .or_else(|| crate::combat_stance::run_anim_for_skel(actor.dat_id))
                            .or_else(|| idle_anim_for_file(actor.dat_id))
                            .map(|a| (a, 1.0))
                    }
                    ClipId::TurnInPlace => {
                        crate::combat_stance::directional_anim_for_skel(actor.dat_id, b"trn")
                            .or_else(|| idle_anim_for_file(actor.dat_id))
                            .map(|a| (a, 1.0))
                    }
                    ClipId::Walk => {
                        crate::combat_stance::directional_anim_for_skel(actor.dat_id, b"wlk")
                            .or_else(|| crate::combat_stance::run_anim_for_skel(actor.dat_id))
                            .or_else(|| idle_anim_for_file(actor.dat_id))
                            .map(|a| (a, 1.0))
                    }
                    ClipId::Idle => idle_anim_for_file(actor.dat_id).map(|a| (a, 1.0)),
                }
            };

        // Advance / start the cross-fade. After this call the blend's
        // `to_clip` is `clip_id`; on a fresh switch `t = 0`; on a
        // stable selection `t` ticks toward 1.
        blends.update(world.id, clip_id, dt);
        let blend = blends.by_id.get(&world.id).copied().expect("just inserted");

        let Some((to_anim, to_scale)) = resolve(blend.to_clip) else {
            continue;
        };
        // While the blend is still in flight, sample both `from` and
        // `to` and lerp per-bone. Once `t >= 1`, skip the `from` sample
        // entirely — saves a DAT lookup + map walk.
        let from_resolved = if blend.t < 1.0 && blend.from_clip != blend.to_clip {
            resolve(blend.from_clip)
        } else {
            None
        };

        // Convert (anim, time_scale_signum) → current frame index. A
        // negative `time_scale` runs the clip backwards (used for the
        // run-as-backpedal fallback).
        let frame_of = |anim: &ffxi_dat::anim::Mo2Animation, scale: f32| -> usize {
            if anim.frames == 0 {
                return 0;
            }
            let safe_speed = if anim.speed > 0.0 { anim.speed } else { 1.0 };
            let t_local = elapsed * scale / safe_speed;
            // Rust % can be negative; wrap into [0, frames).
            let frames = anim.frames as i64;
            let raw = t_local.floor() as i64;
            let idx = ((raw % frames) + frames) % frames;
            idx as usize
        };
        let to_frame = frame_of(&to_anim, to_scale);
        let from_frame = from_resolved
            .as_ref()
            .map(|(a, s)| frame_of(a, *s))
            .unwrap_or(0);

        let blend_t = blend.t.clamp(0.0, 1.0);

        for (i, bone) in raw.bones.iter().enumerate() {
            // Bone[0] carries the `bind_to_bevy` axis flip set up in
            // `spawn_skinned_actor`. Animating it from the MO2 frame
            // would overwrite that flip and rotate the whole skeleton
            // back into FFXI-engine axes — character lays on its side.
            // Skip it; idle anim's root-bone motion is small enough
            // (slight sway/breathing translate) that losing it is
            // invisible vs. the cost of axis corruption.
            if i == 0 {
                continue;
            }
            let Some(&bone_e) = actor.bone_entities.get(i) else {
                continue;
            };

            // Sample `to` first (always live). Then optionally sample
            // `from` and slerp / lerp by `blend_t`.
            let (to_rot, to_trans, to_scale_arr) = match to_anim
                .frames_for_bone(i)
                .and_then(|frames| frames.get(to_frame))
            {
                Some(f) => (f.rotation, f.translation, f.scale),
                None => (bone.rot, bone.trans, [1.0, 1.0, 1.0]),
            };

            let (rot, trans, scale) = match from_resolved.as_ref() {
                Some((from_anim, _)) => {
                    let (from_rot, from_trans, from_scale_arr) = match from_anim
                        .frames_for_bone(i)
                        .and_then(|frames| frames.get(from_frame))
                    {
                        Some(f) => (f.rotation, f.translation, f.scale),
                        None => (bone.rot, bone.trans, [1.0, 1.0, 1.0]),
                    };
                    let q_from =
                        Quat::from_xyzw(from_rot[0], from_rot[1], from_rot[2], from_rot[3]);
                    let q_to = Quat::from_xyzw(to_rot[0], to_rot[1], to_rot[2], to_rot[3]);
                    let q = q_from.slerp(q_to, blend_t);
                    let t_from = Vec3::from_array(from_trans);
                    let t_to = Vec3::from_array(to_trans);
                    let t = t_from.lerp(t_to, blend_t);
                    let s_from = Vec3::from_array(from_scale_arr);
                    let s_to = Vec3::from_array(to_scale_arr);
                    let s = s_from.lerp(s_to, blend_t);
                    ([q.x, q.y, q.z, q.w], [t.x, t.y, t.z], [s.x, s.y, s.z])
                }
                None => (to_rot, to_trans, to_scale_arr),
            };

            if let Ok(mut tf) = q_bones.get_mut(bone_e) {
                tf.rotation = Quat::from_xyzw(rot[0], rot[1], rot[2], rot[3]);
                tf.translation = Vec3::from_array(trans);
                tf.scale = Vec3::from_array(scale);
            }
        }
    }
}

/// Coerce a user-typed clip name into the 3-byte ASCII-lowercase prefix
/// that `load_anim_with_prefix` expects. `"run0"` and `"run"` both
/// resolve to `b"run"`; shorter input is padded with NULs (which will
/// fail to match any real chunk — the model viewer just shows no
/// animation in that case).
fn override_prefix(name: &str) -> [u8; 3] {
    let bytes = name.as_bytes();
    let mut p = [0u8; 3];
    for (i, slot) in p.iter_mut().enumerate() {
        *slot = bytes.get(i).copied().unwrap_or(0).to_ascii_lowercase();
    }
    p
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
/// Returns `Some(min_y_in_parent_frame)` — the lowest local-Y of any
/// baked vertex after the mesh's `bind_to_bevy` rotation is folded in.
/// `None` when the slot loaded but produced no renderable geometry.
/// Callers aggregate this across slots to drive [`crate::scene::BakedActor::min_mesh_y`].
/// Returns `Some((min_local_y, max_local_y))` — the vertical extent
/// of the baked vertices in the mesh entity's *post-rotation* frame,
/// **before** the feet-at-origin translation that's been folded into
/// the mesh entity's spawn transform. Callers aggregate this across
/// slots and use `(min, max)` to fill [`crate::scene::BakedActor`].
/// `feet_translation_y` shifts every spawned mesh entity up by this
/// many yalms in the parent's local frame. The caller is responsible
/// for picking a value that holds **for every slot of the actor** —
/// otherwise legs / head / body slots will shift independently and
/// break inter-slot alignment. `spawn_equipped` aggregates
/// `min_local_y` across all slots and passes the same negation here
/// for every slot. Returns `(min, max)` of the slot's own post-bake
/// y extent for diagnostic reporting / further aggregation.
pub fn spawn_vos2_meshes(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    loaded: &LoadedVos2,
    race: u8,
    feet_translation_y: f32,
) -> Option<(f32, f32)> {
    let baked = baked_skeleton(race);
    spawn_vos2_meshes_with_skeleton(
        commands,
        meshes,
        materials,
        images,
        parent,
        loaded,
        baked.as_ref(),
        feet_translation_y,
    )
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
    feet_translation_y: f32,
) -> Option<(f32, f32)> {
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

    // CPU bind-pose bake. NPCs are dispatched through the GPU
    // `spawn_skinned_actor` path instead — this function now serves
    // only the PC equipment pipeline (launcher preview + in-game PC
    // bodies via `spawn_equipped`). PCs were already correct at the
    // bind pose; an earlier attempt to also pose-bake them against
    // their skeleton DAT's idle MO2 (frame 0) misaligned slot meshes
    // because each slot's mesh was baked against a pose its
    // bone-index palette didn't actually account for. Reverted to
    // pure bind pose here; NPC animation lives entirely on the
    // SkinnedMesh path.
    //
    // `baked_for_mesh` returns None when the skeleton doesn't fit
    // (bone-index out of range), in which case the helpers fall
    // back to local-space rendering — the pre-bake behavior, small
    // and contained at the entity origin.
    let baked = baked_for_mesh(&loaded.mesh, baked_owned);
    // Skeleton-fit diagnostic: knowing whether `baked` is Some vs None
    // explains the "body slot missing" symptom — when fit fails, every
    // vert renders in raw local-space at near-origin coords (invisible
    // inside the parent transform).
    {
        let total_verts = loaded.mesh.vertices.len();
        let weight2 = loaded.mesh.bone_weights.len();
        let weight1 = total_verts.saturating_sub(weight2);
        let skel_n = baked_owned.map(|b| b.world.len()).unwrap_or(0);
        let max_bone1 = loaded
            .mesh
            .bone_indices
            .iter()
            .step_by(2)
            .map(|bi| bi.bone_index1 as usize)
            .max()
            .unwrap_or(0);
        let max_bone2 = loaded
            .mesh
            .bone_indices
            .iter()
            .skip(1)
            .step_by(2)
            .map(|bi| bi.bone_index1 as usize)
            .max()
            .unwrap_or(0);
        let max_table = loaded.mesh.bone_table.iter().copied().max().unwrap_or(0) as usize;
        info!(
            "vos2 fit: skel_bones={} use_bone_table={} bone_table_max={} max_bone1={} max_bone2={} \
             verts={}(rigid={}, w2={}) groups={} baked_skel={}",
            skel_n,
            loaded.mesh.header.use_bone_table(),
            max_table,
            max_bone1,
            max_bone2,
            total_verts,
            weight1,
            weight2,
            loaded.mesh.groups.len(),
            baked.is_some(),
        );
    }
    let positions: Vec<[f32; 3]> = loaded
        .mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let p = bake_position(&loaded.mesh, i, v.pos, baked);
            [p[0], p[2], -p[1]]
        })
        .collect();
    let normals: Vec<[f32; 3]> = loaded
        .mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let n = bake_normal(&loaded.mesh, i, v.normal, baked);
            [n[0], n[2], -n[1]]
        })
        .collect();
    // Diagnostic + bake-extent measurement. The `local_y` of a baked
    // vertex in the mesh entity's *post-rotation* frame is
    // `positions[i][2]` (because `bind_to_bevy = Q_y(π/2) * Q_x(-π/2)`
    // maps mesh-space `(a, b, c)` to `(-b, c, -a)`). Capture both min
    // and max so spawn_one can translate by `-min_local_y` (feet at
    // entity y=0) and the caller can record `actor_height = max - min`
    // for nameplate / camera anchoring.
    let mut min_local_y: Option<f32> = None;
    let mut max_local_y: Option<f32> = None;
    if !positions.is_empty() {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &positions {
            for a in 0..3 {
                if p[a] < min[a] {
                    min[a] = p[a];
                }
                if p[a] > max[a] {
                    max[a] = p[a];
                }
            }
        }
        let extent = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        info!(
            "vos2 bake extent: x=[{:.2}..{:.2}] y=[{:.2}..{:.2}] z=[{:.2}..{:.2}] dx={:.2} dy={:.2} dz={:.2} \
             (largest axis = character's long dimension)",
            min[0], max[0], min[1], max[1], min[2], max[2], extent[0], extent[1], extent[2],
        );
        min_local_y = Some(min[2]);
        max_local_y = Some(max[2]);
    }
    // `feet_translation_y` is supplied by the caller (aggregated across
    // every slot of the actor); see this function's doc-comment.

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
        let (group_roughness, group_metallic) =
            pbr_from_specular(group.specular_exponent, group.specular_intensity);
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
                perceptual_roughness: group_roughness,
                metallic: group_metallic,
                // Same rationale as the vos2 NPC path above: FFXI's
                // baked diffuse textures + Bevy's default 0.5 F0
                // reflectance read as "shiny cloth/skin" — kill it
                // until a proper FFXI-spec→Fresnel pipeline exists.
                reflectance: 0.0,
                cull_mode: None,
                ..default()
            });

            // Two-axis correction from FFXI bind to Bevy convention:
            //   1. Rotate -90° around X so the bake's Bevy +Z
            //      (head-to-feet, per the extent-log diagnostic)
            //      becomes Bevy +Y (Bevy's up axis).
            //   2. Then rotate +90° around Y so the character's
            //      forward direction lands on Bevy -Z (forward),
            //      not -X (camera-left). The π/2 (not π) here is
            //      paired with the `-angle` in
            //      `scene::heading_to_quat`; together they keep the
            //      character facing the same compass direction as
            //      camera/server heading conventions.
            //
            // Composed in Quat multiplication order: outermost (Y)
            // applies last. So `Q_y(π/2) * Q_x(-π/2)` means "first
            // stand the character up, then yaw 90°."
            //
            // Translation `feet_translation_y` shifts the rotated
            // mesh so its lowest vertex sits at the parent entity's
            // local y=0 — that's the snap invariant (entity.y is
            // feet-on-ground). The snap then becomes one line and
            // doesn't need a per-actor offset lookup.
            let bind_to_bevy = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
                * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
            commands.spawn((
                Vos2Overlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                Transform {
                    translation: Vec3::Y * feet_translation_y,
                    rotation: bind_to_bevy,
                    scale: Vec3::ONE,
                },
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
    match (min_local_y, max_local_y) {
        (Some(lo), Some(hi)) => Some((lo, hi)),
        _ => None,
    }
}

/// One loaded equipment/face slot: parsed VOS2 plus a diagnostics
/// label. Produced by [`prepare_equipped`] (Pass 1), consumed by
/// [`spawn_prepared_equipped`] (Pass 2).
pub struct LoadedSlot {
    pub loaded: LoadedVos2,
    pub label: String,
}

/// Result of Pass 1 of the equipment bake: every slot's parsed VOS2
/// plus the actor-wide vertical extent used to anchor feet at y=0.
///
/// Holds only owned, `Send` data (`LoadedVos2` is `Vec`/`String`/POD),
/// so it can be produced off the main thread on an
/// `AsyncComputeTaskPool` task and handed to [`spawn_prepared_equipped`]
/// for the main-thread asset + entity work.
pub struct PreparedEquipped {
    pub slots: Vec<LoadedSlot>,
    pub feet_translation_y: f32,
    pub min_mesh_y: f32,
    pub max_mesh_y: f32,
    pub race: u8,
}

/// Pass 1: resolve the 8 equipment slots + face to DAT files, load and
/// parse every VOS2 chunk, and measure the actor-wide post-bake Y
/// extent. Pure CPU work (file IO + parse + measure) with no Bevy
/// `Commands`/`Assets` — safe to run on a background task. See
/// [`spawn_equipped`] for the combined synchronous path and
/// [`spawn_prepared_equipped`] for the main-thread spawn half.
pub fn prepare_equipped(
    race: u8,
    face: u8,
    head: u16,
    body: u16,
    hands: u16,
    legs: u16,
    feet: u16,
    main: u16,
    sub: u16,
    ranged: u16,
) -> PreparedEquipped {
    use crate::look_resolver::{resolve_equipment_slot, resolve_face};
    let slot_names = [
        "head", "body", "hands", "legs", "feet", "main", "sub", "ranged",
    ];
    let slots = [head, body, hands, legs, feet, main, sub, ranged];

    // ---- Pass 1: load + measure every slot's VOS2 chunks ----
    //
    // We can't decide the actor's feet-at-origin translation until
    // we've seen the deepest min_local_y across **every** slot.
    // Spawning per-slot with each slot's own min was the bug that
    // disassembled characters — legs translated up by their own deep
    // min, head translated up by its shallow min, the two ended up
    // ~1 yalm apart, and the assembled rig collapsed.
    //
    // Each entry holds the loaded VOS2 + a label for diagnostics. We
    // walk it again in pass 2 (`spawn_prepared_equipped`) to spawn meshes.
    let baked_skel = baked_skeleton(race);
    let mut loaded_slots: Vec<LoadedSlot> = Vec::new();
    let mut actor_min_local_y: f32 = f32::INFINITY;
    let mut actor_max_local_y: f32 = f32::NEG_INFINITY;
    let mut load_chunks = |file_id: u32, chunks: Vec<usize>, label: &str| {
        for idx in chunks {
            match load_vos2(file_id, idx) {
                Ok(loaded)
                    if !loaded.mesh.groups.is_empty() && !loaded.mesh.vertices.is_empty() =>
                {
                    if let Some((min_y, max_y)) =
                        measure_post_bake_y_extent(&loaded, baked_skel.as_ref())
                    {
                        // Weapons (main/sub/ranged) routinely tower above
                        // the crown in the bind pose — a polearm/staff is
                        // ~2 yalms grip-to-tip, a bow rides high on the
                        // back. Letting them widen the actor bbox pushes
                        // both the feet anchor and the nameplate crown to
                        // weapon-tip height, which floats the label far
                        // above the head. Measure the body silhouette only;
                        // the weapon meshes still spawn and render, they
                        // just don't drive the anchors.
                        if !matches!(label, "main" | "sub" | "ranged") {
                            actor_min_local_y = actor_min_local_y.min(min_y);
                            actor_max_local_y = actor_max_local_y.max(max_y);
                        }
                    }
                    loaded_slots.push(LoadedSlot {
                        loaded,
                        label: label.to_string(),
                    });
                }
                Ok(_) => info!(
                    "spawn_equipped: {} file={} chunk={} loaded but empty (race={})",
                    label, file_id, idx, race
                ),
                Err(e) => info!(
                    "spawn_equipped: {} file={} chunk={} load failed: {} (race={})",
                    label, file_id, idx, e, race
                ),
            }
        }
    };

    // Face DAT first (lotus loads it alongside the 8 equipment slots).
    if let Some(file_id) = resolve_face(face, race) {
        let chunks = enumerate_vos2_chunks(file_id);
        if chunks.is_empty() {
            info!(
                "spawn_equipped: face file={} has no VOS2 chunks (race={})",
                file_id, race
            );
        } else {
            load_chunks(file_id, chunks, "face");
        }
    }
    for (slot_id, slot_name) in slots.iter().zip(slot_names.iter()) {
        let Some(file_id) = resolve_equipment_slot(*slot_id, race) else {
            if *slot_id != 0 {
                info!(
                    "spawn_equipped: slot {}={:#06X} unresolved (race={})",
                    slot_name, slot_id, race
                );
            }
            continue;
        };
        let chunks = enumerate_vos2_chunks(file_id);
        if chunks.is_empty() {
            info!(
                "spawn_equipped: slot {} file={} no VOS2 chunks (slot_id={:#06X} race={})",
                slot_name, file_id, slot_id, race
            );
            continue;
        }
        let label = format!("slot {}", slot_name);
        load_chunks(file_id, chunks, &label);
    }

    // Pass 1 done. Compute the actor-wide feet anchor. An empty slot
    // list falls through with the (-0.9, 1.6) fallback extent; Pass 2
    // (`spawn_prepared_equipped`) returns 0 for it.
    //
    // `feet_translation_y` lifts every slot's geometry so the deepest
    // baked body vertex (across the whole actor) sits at the parent
    // entity's local y=0 — the snap-invariant feet position. Every
    // slot uses the same number; inter-slot relative positions are
    // preserved.
    let (min_mesh_y, max_mesh_y) = if actor_min_local_y.is_finite() && actor_max_local_y.is_finite()
    {
        (actor_min_local_y, actor_max_local_y)
    } else {
        (-0.9, 1.6)
    };
    let feet_translation_y = -min_mesh_y;
    PreparedEquipped {
        slots: loaded_slots,
        feet_translation_y,
        min_mesh_y,
        max_mesh_y,
        race,
    }
}

/// Pass 2: spawn Bevy mesh entities for a [`PreparedEquipped`] under
/// `parent`, lifting every slot by the shared `feet_translation_y`.
/// Must run on the main thread (touches `Assets` + spawns entities).
///
/// Returns the number of slots that actually produced geometry — the
/// launcher can use this to decide whether to fall back to a
/// placeholder when the spawn was a total miss.
pub fn spawn_prepared_equipped(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    prepared: &PreparedEquipped,
) -> usize {
    use crate::scene::BakedActor;
    let mut spawned = 0usize;
    for slot in &prepared.slots {
        if spawn_vos2_meshes(
            commands,
            meshes,
            materials,
            images,
            parent,
            &slot.loaded,
            prepared.race,
            prepared.feet_translation_y,
        )
        .is_some()
        {
            spawned += 1;
        }
        let _ = &slot.label; // present for log/debug attachment
    }

    if spawned > 0 {
        let actor_height = (prepared.max_mesh_y - prepared.min_mesh_y).max(0.1);
        // `try_insert`: `parent` is caller-supplied and may be an in-game
        // actor that despawned before this flush. Tolerate that race.
        commands.entity(parent).try_insert(BakedActor {
            min_mesh_y: prepared.min_mesh_y,
            actor_height,
        });
    }
    spawned
}

/// Compose: resolve each of the 8 equipment slots to a DAT file, load
/// each VOS2 chunk, and spawn it under `parent` with the race's
/// bind-pose skeleton applied. Slots set to `0`-id sentinels are
/// silently skipped (no item equipped).
///
/// Combined synchronous bake: [`prepare_equipped`] (Pass 1) then
/// [`spawn_prepared_equipped`] (Pass 2) back-to-back on the calling
/// thread. Retained for callers that bake on the main thread (the
/// char-create live preview). The char-list preview instead runs
/// `prepare_equipped` on a background task and polls the result — see
/// `launcher_ui::char_preview`.
pub fn spawn_equipped(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    race: u8,
    face: u8,
    head: u16,
    body: u16,
    hands: u16,
    legs: u16,
    feet: u16,
    main: u16,
    sub: u16,
    ranged: u16,
) -> usize {
    let prepared = prepare_equipped(race, face, head, body, hands, legs, feet, main, sub, ranged);
    spawn_prepared_equipped(commands, meshes, materials, images, parent, &prepared)
}

/// Translate FFXI Phong specular params to Bevy PBR `(roughness,
/// metallic)`. No reference mapping exists — lotus stores the raw
/// f32s and lets its Vulkan pipeline interpret them.
///
/// Intentionally does *not* map `specular_intensity` to `metallic`:
/// the FFXI intensity field is a Phong-scalar, not a "this material
/// is metal" flag. Feeding it into Bevy's `metallic` made ordinary
/// cloth gear render as polished steel ("shiny/reflective
/// characters seems wrong" — user feedback). Until a proper
/// metal-flag pipeline lands we only modulate roughness from
/// exponent and leave metallic at 0.
fn pbr_from_specular(exponent: f32, _intensity: f32) -> (f32, f32) {
    let roughness = if exponent <= 0.0 {
        1.0
    } else {
        // Higher exponent → tighter highlight → less rough. Floor
        // at 0.3 (vs prior 0.1) so the sharpest highlights still
        // read as "cloth with sheen" rather than "wet plastic."
        (1.0 - (exponent.ln_1p() / 5.0)).clamp(0.3, 1.0)
    };
    (roughness, 0.0)
}
