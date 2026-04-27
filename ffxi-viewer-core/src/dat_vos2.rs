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

use crate::graphics_settings::{CharacterRenderPath, GraphicsSettings};
use crate::scene::{BakedActor, TrackedEntities};
use crate::skeleton_instance::{
    eval_bind_pose, eval_pose, pc_pivot_rotation, pc_pivot_translation, FfxiActor,
};
use crate::skinned_ffxi_material::{
    FfxiJointMatrices, FfxiLightingUniform, FfxiMaterialFlags, FfxiSkinnedMaterial, ATTR_COLOR,
    ATTR_JOINT0, ATTR_JOINT1, ATTR_JOINT_WEIGHT, ATTR_NORMAL0, ATTR_NORMAL1, ATTR_POSITION0,
    ATTR_POSITION1,
};

#[derive(Component, Debug)]
pub struct SkinnedActor {
    pub dat_id: u32,

    pub bone_entities: Vec<Entity>,

    pub pivot: Entity,

    pub min_local_y: f32,

    pub max_local_y: f32,
}

#[derive(Component)]
pub struct Vos2Overlay;

#[derive(Message, Debug, Clone, Copy)]
pub struct LoadVos2Request {
    pub file_id: u32,
    pub chunk_idx: usize,
    pub entity_id: u32,

    pub race: u8,

    pub skeleton_file_id: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Vos2NamedTexture {
    pub name: String,
    pub texture: DecodedTexture,
}

pub struct LoadedVos2 {
    pub mesh: Vos2Mesh,
    pub textures: Vec<Vos2NamedTexture>,
}

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

fn top_container<'r, 'a>(tree: &'r ChunkNode<'a>) -> &'r ChunkNode<'a> {
    tree.children.first().unwrap_or(tree)
}

fn has_vos2_recursive(node: &ChunkNode<'_>) -> bool {
    ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::VertexOs2)
        || node.children.iter().any(has_vos2_recursive)
}

pub fn dat_has_skinned_mesh(file_id: u32) -> bool {
    let Ok(root) = DatRoot::from_env_or_default() else {
        return false;
    };
    let Ok(loc) = root.resolve(file_id) else {
        return false;
    };
    let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
        return false;
    };
    has_vos2_recursive(&walk_tree(&bytes))
}

pub fn load_vos2(file_id: u32, chunk_idx: usize) -> Result<LoadedVos2, String> {
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let tree = walk_tree(&bytes);
    let container = top_container(&tree);
    let os2_children: Vec<&ChunkNode<'_>> = container
        .children
        .iter()
        .filter(|n| ChunkKind::from_u8(n.chunk.kind) == Some(ChunkKind::VertexOs2))
        .collect();

    let node = match os2_children.get(chunk_idx) {
        Some(n) => *n,
        None => os2_children
            .iter()
            .copied()
            .max_by_key(|n| n.chunk.data.len())
            .ok_or_else(|| format!("no VertexOs2 child of root Rmp in file {file_id}"))?,
    };
    let mesh = parse_vos2(node.chunk.data).map_err(|e| format!("vos2 parse: {e}"))?;

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

const PC_SKELETON_FILE_IDS: [u32; 8] = [7072, 10248, 13424, 16600, 19776, 19776, 23176, 26352];

pub fn skeleton_file_id_for_race(race: u8) -> Option<u32> {
    let idx = race.checked_sub(1)? as usize;
    PC_SKELETON_FILE_IDS.get(idx).copied()
}

static BAKED_SKELETONS: OnceLock<
    std::sync::Mutex<std::collections::HashMap<u32, Option<BakedSkeleton>>>,
> = OnceLock::new();

#[derive(Clone)]
struct BakedSkeleton {
    world: Vec<[[f32; 4]; 4]>,

    raw: Option<std::sync::Arc<ffxi_dat::bone::Skeleton>>,
}

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

    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY);
    for bone in &world {
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

fn load_idle_animation_for_file(file_id: u32) -> Option<ffxi_dat::anim::Mo2Animation> {
    let root = DatRoot::from_env_or_default().ok()?;
    let loc = root.resolve(file_id).ok()?;
    let bytes = fs::read(loc.path_under(root.root())).ok()?;
    for chunk in walk(&bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }

        let prefix = &chunk.name[..3];
        if prefix.eq_ignore_ascii_case(b"idl") {
            if let Ok(anim) = ffxi_dat::anim::parse_mo2(chunk.data, &chunk.name) {
                return Some(anim);
            }
        }
    }
    None
}

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

fn baked_skeleton(race: u8) -> Option<BakedSkeleton> {
    let file_id = skeleton_file_id_for_race(race)?;
    baked_skeleton_for_file(file_id)
}

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

fn baked_for_mesh<'a>(
    mesh: &Vos2Mesh,
    baked: Option<&'a BakedSkeleton>,
) -> Option<&'a BakedSkeleton> {
    baked.filter(|b| skeleton_fits_mesh(b, mesh))
}

fn unroll_root_rotation(v: [f32; 3]) -> [f32; 3] {
    [v[2], v[1], -v[0]]
}

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

        if let (Some(m1), Some(m2)) = (m1, m2) {
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

        if let Some(m1) = m1 {
            return unroll_root_rotation(bone::mat4_transform_point(*m1, bw.pos1));
        }
        return bw.pos1;
    }

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

pub fn process_load_vos2_requests(
    mut events: MessageReader<LoadVos2Request>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut inverse_bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    tracked: Res<TrackedEntities>,

    q_skinned_actor: Query<&SkinnedActor>,

    q_baked: Query<&crate::scene::BakedActor>,

    mut q_xform: Query<&mut Transform>,
    settings: Res<GraphicsSettings>,
) {
    if settings.character_path() == CharacterRenderPath::FfxiFaithful {
        return;
    }
    let queued: Vec<LoadVos2Request> = events.read().copied().collect();
    if queued.is_empty() {
        return;
    }

    let mut load_cache: std::collections::HashMap<(u32, usize), Option<LoadedVos2>> =
        std::collections::HashMap::new();
    let mut despawned: std::collections::HashSet<u32> = std::collections::HashSet::new();

    let mut actor_state: std::collections::HashMap<u32, (Vec<Entity>, Entity, f32, f32)> =
        std::collections::HashMap::new();

    let mut cpu_extent: std::collections::HashMap<u32, (f32, f32)> =
        std::collections::HashMap::new();

    for req in queued {
        let Some(&bevy_e) = tracked.by_id.get(&req.entity_id) else {
            continue;
        };
        let entry = load_cache
            .entry((req.file_id, req.chunk_idx))
            .or_insert_with(|| load_vos2(req.file_id, req.chunk_idx).ok());
        let Some(loaded) = entry.as_ref() else {
            continue;
        };
        if loaded.mesh.groups.is_empty() || loaded.mesh.vertices.is_empty() {
            continue;
        }

        if despawned.insert(req.entity_id) {
            commands.entity(bevy_e).remove::<Mesh3d>();
        }

        let baked_owned = match req.skeleton_file_id {
            Some(id) => baked_skeleton_for_file(id),
            None => baked_skeleton(req.race),
        };

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

                let (slot_min, slot_max) = if is_pc {
                    measure_post_bake_y_extent(loaded, baked_owned.as_ref()).unwrap_or((0.0, 1.9))
                } else {
                    compute_skinned_local_y_extent(loaded, is_pc).unwrap_or((-0.9, 1.6))
                };

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

                commands.entity(bevy_e).try_insert(SkinnedActor {
                    dat_id: raw_dat_id_for_skeleton(raw),
                    bone_entities,
                    pivot,
                    min_local_y: actor_min_y,
                    max_local_y: actor_max_y,
                });
                commands
                    .entity(bevy_e)
                    .try_insert(crate::scene::BakedActor {
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

        if req.race != 0 {
            if let Some((slot_min, slot_max)) = slot_extent {
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

                commands
                    .entity(bevy_e)
                    .try_insert(crate::scene::BakedActor {
                        min_mesh_y: merged_min,
                        actor_height: (merged_max - merged_min).max(0.1),
                    });
            }
        }
    }
}

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

    let actor_min_y = {
        let mut mn = f32::INFINITY;

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

fn spawn_skinned_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    parent: Entity,
    loaded: &LoadedVos2,
    raw: &std::sync::Arc<Skeleton>,

    existing_actor: Option<(Vec<Entity>, Entity)>,
    is_pc: bool,

    min_local_y: f32,
) -> (Vec<Entity>, Entity) {
    use ffxi_dat::bone::PARENT_ROOT;

    let (bone_entities, pivot) = match existing_actor {
        Some((bones, pivot)) => (bones, pivot),
        None => {
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

    let n = loaded.mesh.vertices.len();
    let weight2_count = loaded.mesh.bone_weights.len();
    let weight1_count = n.saturating_sub(weight2_count);
    let mut joint_indices: Vec<[u16; 4]> = vec![[0u16; 4]; n];
    let mut joint_weights: Vec<[f32; 4]> = vec![[1.0, 0.0, 0.0, 0.0]; n];
    let mut out_of_range_count = 0usize;
    for i in 0..n {
        let bone1 = loaded.mesh.skeleton_bone_for(i).unwrap_or(0);
        if (bone1 as usize) >= raw.bones.len() {
            joint_indices[i][0] = 0;
            joint_weights[i] = [1.0, 0.0, 0.0, 0.0];
            out_of_range_count += 1;
            continue;
        }
        joint_indices[i][0] = bone1;

        if i >= weight1_count {
            let k = i - weight1_count;
            let bw = &loaded.mesh.bone_weights[k];
            let bone2 = loaded.mesh.skeleton_bone2_for(i).unwrap_or(0);
            let bone2_valid = (bone2 as usize) < raw.bones.len();
            let (w1, w2) = if bone2_valid {
                joint_indices[i][1] = bone2;
                (bw.weight1, bw.weight2)
            } else {
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
            metallic: 0.0,

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

struct FfxiVertexData {
    position0: Vec<[f32; 3]>,
    position1: Vec<[f32; 3]>,
    normal0: Vec<[f32; 3]>,
    normal1: Vec<[f32; 3]>,
    weight: Vec<f32>,
    joint0: Vec<u32>,
    joint1: Vec<u32>,
    color: Vec<[f32; 4]>,
}

fn flip_axis(v: [f32; 3], axis: u8) -> [f32; 3] {
    match axis {
        1 => [-v[0], v[1], v[2]],
        2 => [v[0], -v[1], v[2]],
        3 => [v[0], v[1], -v[2]],
        _ => v,
    }
}

fn build_ffxi_vertex_data(mesh: &Vos2Mesh, skel_bones: usize) -> (FfxiVertexData, bool) {
    let n = mesh.vertices.len();
    let weight2_count = mesh.bone_weights.len();
    let weight1_count = n.saturating_sub(weight2_count);
    let mirrored = mesh.header.flip != 0;
    let total = if mirrored { n * 2 } else { n };

    let mut vd = FfxiVertexData {
        position0: vec![[0.0; 3]; total],
        position1: vec![[0.0; 3]; total],
        normal0: vec![[0.0; 3]; total],
        normal1: vec![[0.0; 3]; total],
        weight: vec![1.0; total],
        joint0: vec![0; total],
        joint1: vec![0; total],
        color: vec![[1.0, 1.0, 1.0, 1.0]; total],
    };

    let clamp_bone = |b: Option<u16>| -> u32 {
        match b {
            Some(b) if (b as usize) < skel_bones => b as u32,
            _ => 0,
        }
    };

    for i in 0..n {
        let b0 = clamp_bone(mesh.skeleton_bone_for(i));
        vd.joint0[i] = b0;
        if i < weight1_count {
            vd.position0[i] = mesh.vertices[i].pos;
            vd.normal0[i] = mesh.vertices[i].normal;
            vd.joint1[i] = b0;
            vd.weight[i] = 1.0;
        } else {
            let bw = &mesh.bone_weights[i - weight1_count];
            vd.position0[i] = bw.pos1;
            vd.normal0[i] = bw.normal1;
            let raw_b1 = mesh.skeleton_bone2_for(i);
            let b1_valid = raw_b1.map(|b| (b as usize) < skel_bones).unwrap_or(false);
            if b1_valid {
                vd.position1[i] = bw.pos2;
                vd.normal1[i] = bw.normal2;
                vd.joint1[i] = raw_b1.unwrap() as u32;
                let sum = bw.weight1 + bw.weight2;
                vd.weight[i] = if sum > 0.0 { bw.weight1 / sum } else { 1.0 };
            } else {
                vd.joint1[i] = b0;
                vd.weight[i] = 1.0;
            }
        }
    }

    if mirrored {
        let indirect = |raw: u8| -> Option<u16> {
            if mesh.header.use_bone_table() {
                mesh.bone_table.get(raw as usize).copied()
            } else {
                Some(raw as u16)
            }
        };
        for i in 0..n {
            let m = n + i;
            let jr0 = mesh.bone_indices.get(i * 2);
            let jr1 = mesh.bone_indices.get(i * 2 + 1);
            let axis0 = jr0.map(|r| r.mirror_axis).unwrap_or(0);
            let axis1 = jr1.map(|r| r.mirror_axis).unwrap_or(0);
            vd.position0[m] = flip_axis(vd.position0[i], axis0);
            vd.position1[m] = flip_axis(vd.position1[i], axis1);
            vd.normal0[m] = flip_axis(vd.normal0[i], axis0);
            vd.normal1[m] = flip_axis(vd.normal1[i], axis1);
            vd.weight[m] = vd.weight[i];
            vd.color[m] = vd.color[i];
            vd.joint0[m] = clamp_bone(jr0.and_then(|r| indirect(r.bone_index2)));
            vd.joint1[m] = clamp_bone(jr1.and_then(|r| indirect(r.bone_index2)));
        }
    }

    (vd, mirrored)
}

#[allow(clippy::too_many_arguments)]
fn spawn_ffxi_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    parent: Entity,
    loaded: &LoadedVos2,
    skeleton: &std::sync::Arc<Skeleton>,
    existing: Option<Entity>,
    is_pc: bool,

    _min_local_y: f32,
) -> (Entity, Vec<Handle<FfxiSkinnedMaterial>>) {
    let pivot = match existing {
        Some(p) => p,
        None => {
            let rotation = if is_pc {
                pc_pivot_rotation()
            } else {
                Quat::from_rotation_x(std::f32::consts::PI)
            };
            let translation = pc_pivot_translation();
            commands
                .spawn((
                    Transform {
                        translation,
                        rotation,
                        scale: Vec3::ONE,
                    },
                    GlobalTransform::default(),
                    Visibility::default(),
                    ChildOf(parent),
                ))
                .id()
        }
    };

    let mut bind_joints = FfxiJointMatrices::default();
    bind_joints.set_from(&eval_bind_pose(skeleton));

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

    let (vd, mirrored) = build_ffxi_vertex_data(&loaded.mesh, skeleton.bones.len());
    let n = loaded.mesh.vertices.len();
    let total = vd.position0.len();
    let mut out_materials = Vec::new();

    for group in &loaded.mesh.groups {
        if group.triangles.is_empty() {
            continue;
        }
        let mut uvs: Vec<[f32; 2]> = vec![[0.0, 0.0]; total];
        let mut uv_set: Vec<bool> = vec![false; total];
        let tri_factor = if mirrored { 2 } else { 1 };
        let mut indices: Vec<u32> = Vec::with_capacity(group.triangles.len() * 3 * tri_factor);
        for t in &group.triangles {
            for c in 0..3 {
                let i = t.indices[c] as usize;
                if i < n && !uv_set[i] {
                    uvs[i] = t.uvs[c];
                    uv_set[i] = true;
                }
                indices.push(t.indices[c] as u32);
            }

            if mirrored {
                for c in 0..3 {
                    let i = t.indices[c] as usize;
                    let mi = i + n;
                    if mi < total && !uv_set[mi] {
                        uvs[mi] = t.uvs[c];
                        uv_set[mi] = true;
                    }
                    indices.push((t.indices[c] as u32) + n as u32);
                }
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
        mesh.insert_attribute(ATTR_POSITION0, vd.position0.clone());
        mesh.insert_attribute(ATTR_POSITION1, vd.position1.clone());
        mesh.insert_attribute(ATTR_NORMAL0, vd.normal0.clone());
        mesh.insert_attribute(ATTR_NORMAL1, vd.normal1.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(ATTR_JOINT_WEIGHT, vd.weight.clone());

        mesh.insert_attribute(
            ATTR_JOINT0,
            bevy::mesh::VertexAttributeValues::Uint32(vd.joint0.clone()),
        );
        mesh.insert_attribute(
            ATTR_JOINT1,
            bevy::mesh::VertexAttributeValues::Uint32(vd.joint1.clone()),
        );
        mesh.insert_attribute(ATTR_COLOR, vd.color.clone());
        mesh.insert_indices(Indices::U32(indices));

        let mat = materials.add(FfxiSkinnedMaterial::new(
            pivot.to_bits(),
            FfxiLightingUniform::default(),
            tex_handle,
            bind_joints.clone(),
            FfxiMaterialFlags::default(),
        ));
        out_materials.push(mat.clone());

        commands.spawn((
            Vos2Overlay,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(mat),
            Transform::default(),
            ChildOf(pivot),
        ));
    }

    (pivot, out_materials)
}

#[allow(clippy::too_many_arguments)]
pub fn process_load_vos2_requests_ffxi(
    mut events: MessageReader<LoadVos2Request>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
    settings: Res<GraphicsSettings>,
    tracked: Res<TrackedEntities>,
    q_actor: Query<&FfxiActor>,
    mut q_xform: Query<&mut Transform>,
) {
    if settings.character_path() != CharacterRenderPath::FfxiFaithful {
        return;
    }
    let queued: Vec<LoadVos2Request> = events.read().copied().collect();
    if queued.is_empty() {
        return;
    }
    let mut load_cache: std::collections::HashMap<(u32, usize), Option<LoadedVos2>> =
        std::collections::HashMap::new();
    let mut despawned: std::collections::HashSet<u32> = std::collections::HashSet::new();

    type ActorAcc = (Entity, f32, f32, Vec<Handle<FfxiSkinnedMaterial>>);
    let mut actor_state: std::collections::HashMap<u32, ActorAcc> =
        std::collections::HashMap::new();

    for req in queued {
        let Some(&bevy_e) = tracked.by_id.get(&req.entity_id) else {
            continue;
        };
        let entry = load_cache
            .entry((req.file_id, req.chunk_idx))
            .or_insert_with(|| load_vos2(req.file_id, req.chunk_idx).ok());
        let Some(loaded) = entry.as_ref() else {
            continue;
        };
        if loaded.mesh.groups.is_empty() || loaded.mesh.vertices.is_empty() {
            continue;
        }

        let baked = match req.skeleton_file_id {
            Some(id) => baked_skeleton_for_file(id),
            None => baked_skeleton(req.race),
        };
        let Some(baked) = baked else {
            continue;
        };
        let Some(skeleton) = baked.raw.as_ref() else {
            continue;
        };

        if despawned.insert(req.entity_id) {
            commands.entity(bevy_e).remove::<Mesh3d>();
        }

        let is_pc = req.race != 0;
        let (slot_min, slot_max) = if is_pc {
            measure_post_bake_y_extent(loaded, Some(&baked)).unwrap_or((0.0, 1.9))
        } else {
            compute_skinned_local_y_extent(loaded, is_pc).unwrap_or((-0.9, 1.6))
        };

        let existing = actor_state
            .get(&req.entity_id)
            .map(|(p, _, _, _)| *p)
            .or_else(|| q_actor.get(bevy_e).ok().map(|a| a.pivot));
        let (cur_min, cur_max, mut mats_acc) = actor_state
            .get(&req.entity_id)
            .map(|(_, mn, mx, m)| (*mn, *mx, m.clone()))
            .or_else(|| {
                q_actor
                    .get(bevy_e)
                    .ok()
                    .map(|a| (a.min_local_y, a.max_local_y, a.materials.clone()))
            })
            .unwrap_or((f32::INFINITY, f32::NEG_INFINITY, Vec::new()));

        let (pivot, new_mats) = spawn_ffxi_actor(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            bevy_e,
            loaded,
            skeleton,
            existing,
            is_pc,
            slot_min,
        );
        mats_acc.extend(new_mats);

        let actor_min = cur_min.min(slot_min);
        let actor_max = cur_max.max(slot_max);
        if let Ok(mut piv) = q_xform.get_mut(pivot) {
            piv.translation.y = -actor_min;
        }
        actor_state.insert(
            req.entity_id,
            (pivot, actor_min, actor_max, mats_acc.clone()),
        );

        let actor_height = (actor_max - actor_min).max(0.1);
        commands.entity(bevy_e).try_insert(FfxiActor {
            skeleton: skeleton.clone(),

            dat_id: raw_dat_id_for_skeleton(skeleton),
            pivot,
            materials: mats_acc,
            min_local_y: actor_min,
            max_local_y: actor_max,
        });
        commands.entity(bevy_e).try_insert(BakedActor {
            min_mesh_y: actor_min,
            actor_height,
        });
    }
}

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

    0
}

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
    let elapsed = time.elapsed_secs();
    let dt = time.delta_secs();
    let bt_target_by_id = bt_target_index(&state);

    for (world, actor) in &q_actors {
        let Some(baked) = baked_skeleton_for_file(actor.dat_id) else {
            continue;
        };
        let Some(raw) = baked.raw else { continue };
        let is_self = state
            .snapshot
            .self_char_id
            .map(|sid| sid == world.id)
            .unwrap_or(false);

        let pose = sample_animation_pose(
            &raw,
            actor.dat_id,
            world.id,
            is_self,
            elapsed,
            dt,
            &motion,
            &rest,
            &mut blends,
            clip_override.as_deref(),
            &bt_target_by_id,
        );

        for (i, bone) in raw.bones.iter().enumerate() {
            if i == 0 {
                continue;
            }
            let Some(&bone_e) = actor.bone_entities.get(i) else {
                continue;
            };
            let (rot, trans, scale) = match &pose[i] {
                Some(bl) => (bl.rotation, bl.translation, bl.scale),
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

pub fn tick_ffxi_actors(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    motion: Res<crate::combat_stance::EntityMotion>,
    rest: Res<crate::combat_stance::RestStance>,
    mut blends: ResMut<crate::combat_stance::AnimationBlends>,
    clip_override: Option<Res<crate::combat_stance::ModelViewerClipOverride>>,
    settings: Res<GraphicsSettings>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    q_actors: Query<(&crate::components::WorldEntity, &FfxiActor)>,
) {
    if settings.character_path() != CharacterRenderPath::FfxiFaithful {
        return;
    }
    let elapsed = time.elapsed_secs();
    let dt = time.delta_secs();
    let bt_target_by_id = bt_target_index(&state);

    for (world, actor) in &q_actors {
        let is_self = state
            .snapshot
            .self_char_id
            .map(|sid| sid == world.id)
            .unwrap_or(false);

        let pose = sample_animation_pose(
            &actor.skeleton,
            actor.dat_id,
            world.id,
            is_self,
            elapsed,
            dt,
            &motion,
            &rest,
            &mut blends,
            clip_override.as_deref(),
            &bt_target_by_id,
        );

        let mats = eval_pose(&actor.skeleton, &pose);
        for h in &actor.materials {
            if let Some(m) = materials.get_mut(h) {
                m.joints.set_from(&mats);
            }
        }
    }
}

pub fn update_ffxi_lighting_system(
    settings: Res<GraphicsSettings>,
    ambient: Res<GlobalAmbientLight>,
    q_sun: Query<
        (&DirectionalLight, &GlobalTransform),
        (
            With<crate::sun_moon::IsSun>,
            Without<crate::sun_moon::IsMoon>,
        ),
    >,
    q_moon: Query<
        (&DirectionalLight, &GlobalTransform),
        (
            With<crate::sun_moon::IsMoon>,
            Without<crate::sun_moon::IsSun>,
        ),
    >,
    q_actors: Query<&FfxiActor>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    if settings.character_path() != CharacterRenderPath::FfxiFaithful {
        return;
    }

    const AMBIENT_REF_LUX: f32 = 1000.0;
    const DIR_REF_LUX: f32 = 12000.0;

    let amb = ambient.color.to_linear();
    let amb_k = (ambient.brightness / AMBIENT_REF_LUX).clamp(0.0, 1.5);
    let ambient_v = Vec4::new(amb.red * amb_k, amb.green * amb_k, amb.blue * amb_k, 1.0);

    let extract = |opt: Option<(&DirectionalLight, &GlobalTransform)>| -> (Vec4, Vec4) {
        match opt {
            Some((dl, gt)) if dl.illuminance > 0.0 => {
                let f = gt.forward();
                let c = dl.color.to_linear();
                let k = (dl.illuminance / DIR_REF_LUX).clamp(0.0, 1.0);
                (
                    Vec4::new(f.x, f.y, f.z, 0.0),
                    Vec4::new(c.red, c.green, c.blue, k),
                )
            }
            _ => (Vec4::ZERO, Vec4::ZERO),
        }
    };
    let (dir0_dir, dir0_color) = extract(q_sun.single().ok());
    let (dir1_dir, dir1_color) = extract(q_moon.single().ok());

    let lighting = FfxiLightingUniform {
        ambient: ambient_v,
        dir0_dir,
        dir0_color,
        dir1_dir,
        dir1_color,

        point_pos: [Vec4::ZERO; 4],
        point_color: [Vec4::ZERO; 4],
        point_atten: [Vec4::ZERO; 4],
    };

    for actor in &q_actors {
        for h in &actor.materials {
            if let Some(m) = materials.get_mut(h) {
                m.lighting = lighting.clone();
            }
        }
    }
}

fn bt_target_index(state: &crate::snapshot::SceneState) -> std::collections::HashMap<u32, u32> {
    state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.bt_target_id))
        .collect()
}

const ANIM_FPS: f32 = 30.0;

#[allow(clippy::too_many_arguments)]
fn sample_animation_pose(
    raw: &Skeleton,
    dat_id: u32,
    world_id: u32,
    is_self: bool,
    elapsed: f32,
    dt: f32,
    motion: &crate::combat_stance::EntityMotion,
    rest: &crate::combat_stance::RestStance,
    blends: &mut crate::combat_stance::AnimationBlends,
    clip_override: Option<&crate::combat_stance::ModelViewerClipOverride>,
    bt_target_by_id: &std::collections::HashMap<u32, u32>,
) -> Vec<Option<bone::BoneLocal>> {
    use crate::combat_stance::{ClipId, EntityMotion};
    let n = raw.bones.len();
    let mut out: Vec<Option<bone::BoneLocal>> = vec![None; n];

    let fill = |out: &mut [Option<bone::BoneLocal>],
                anim: &ffxi_dat::anim::Mo2Animation,
                frame_idx: usize| {
        for (i, slot) in out.iter_mut().enumerate().skip(1) {
            if let Some(f) = anim.frames_for_bone(i).and_then(|fr| fr.get(frame_idx)) {
                *slot = Some(bone::BoneLocal {
                    rotation: f.rotation,
                    translation: f.translation,
                    scale: f.scale,
                });
            }
        }
    };

    let wrap_frame = |anim: &ffxi_dat::anim::Mo2Animation| -> usize {
        if anim.frames == 0 {
            return 0;
        }
        let safe_speed = if anim.speed > 0.0 { anim.speed } else { 1.0 };
        ((elapsed * ANIM_FPS * safe_speed).floor() as usize) % anim.frames as usize
    };

    if let Some(over) = clip_override {
        let prefix = override_prefix(&over.clip_name);
        if let Some(anim) = crate::combat_stance::override_anim_for_skel(dat_id, &prefix) {
            if anim.frames > 0 {
                fill(&mut out, &anim, wrap_frame(&anim));
            }
        }
        return out;
    }

    if is_self {
        use crate::combat_stance::RestKind;
        let rest_anim = match rest.kind {
            RestKind::Sit => crate::combat_stance::sit_anim_for_skel(dat_id)
                .or_else(|| idle_anim_for_file(dat_id)),
            RestKind::Heal => crate::combat_stance::heal_anim_for_skel(dat_id)
                .or_else(|| crate::combat_stance::sit_anim_for_skel(dat_id))
                .or_else(|| idle_anim_for_file(dat_id)),
            RestKind::None => None,
        };
        if let Some(anim) = rest_anim {
            if anim.frames > 0 {
                fill(&mut out, &anim, wrap_frame(&anim));
                return out;
            }
        }
    }

    let engaged = bt_target_by_id
        .get(&world_id)
        .map(|&t| t != 0)
        .unwrap_or(false);
    let sample = motion.sample(world_id).unwrap_or_default();
    let moving = sample.speed > EntityMotion::MOVE_THRESHOLD;

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

    let resolve = |clip: ClipId| -> Option<(std::sync::Arc<ffxi_dat::anim::Mo2Animation>, f32)> {
        match clip {
            ClipId::CombatRun => crate::combat_stance::combat_run_anim_for_skel(dat_id)
                .or_else(|| crate::combat_stance::run_anim_for_skel(dat_id))
                .or_else(|| crate::combat_stance::battle_idle_anim_for_skel(dat_id))
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::BattleIdle => crate::combat_stance::battle_idle_anim_for_skel(dat_id)
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::Run => crate::combat_stance::run_anim_for_skel(dat_id)
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::Backpedal => crate::combat_stance::directional_anim_for_skel(dat_id, b"bck")
                .map(|a| (a, 1.0))
                .or_else(|| crate::combat_stance::run_anim_for_skel(dat_id).map(|a| (a, -1.0)))
                .or_else(|| idle_anim_for_file(dat_id).map(|a| (a, 1.0))),
            ClipId::StrafeLeft => crate::combat_stance::directional_anim_for_skel(dat_id, b"stl")
                .or_else(|| crate::combat_stance::run_anim_for_skel(dat_id))
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::StrafeRight => crate::combat_stance::directional_anim_for_skel(dat_id, b"str")
                .or_else(|| crate::combat_stance::run_anim_for_skel(dat_id))
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::TurnInPlace => crate::combat_stance::directional_anim_for_skel(dat_id, b"trn")
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::Walk => crate::combat_stance::directional_anim_for_skel(dat_id, b"wlk")
                .or_else(|| crate::combat_stance::run_anim_for_skel(dat_id))
                .or_else(|| idle_anim_for_file(dat_id))
                .map(|a| (a, 1.0)),
            ClipId::Idle => idle_anim_for_file(dat_id).map(|a| (a, 1.0)),
        }
    };

    blends.update(world_id, clip_id, dt);
    let blend = blends.by_id.get(&world_id).copied().expect("just inserted");

    let Some((to_anim, to_scale)) = resolve(blend.to_clip) else {
        return out;
    };

    let from_resolved = if blend.t < 1.0 && blend.from_clip != blend.to_clip {
        resolve(blend.from_clip)
    } else {
        None
    };

    let frame_of = |anim: &ffxi_dat::anim::Mo2Animation, scale: f32| -> usize {
        if anim.frames == 0 {
            return 0;
        }
        let safe_speed = if anim.speed > 0.0 { anim.speed } else { 1.0 };
        let t_local = elapsed * ANIM_FPS * safe_speed * scale;

        let frames = anim.frames as i64;
        let r = t_local.floor() as i64;
        (((r % frames) + frames) % frames) as usize
    };
    let to_frame = frame_of(&to_anim, to_scale);
    let from_frame = from_resolved
        .as_ref()
        .map(|(a, s)| frame_of(a, *s))
        .unwrap_or(0);
    let blend_t = blend.t.clamp(0.0, 1.0);

    for (i, bone) in raw.bones.iter().enumerate().skip(1) {
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
                let q_from = Quat::from_xyzw(from_rot[0], from_rot[1], from_rot[2], from_rot[3]);
                let q_to = Quat::from_xyzw(to_rot[0], to_rot[1], to_rot[2], to_rot[3]);
                let q = q_from.slerp(q_to, blend_t);
                let t = Vec3::from_array(from_trans).lerp(Vec3::from_array(to_trans), blend_t);
                let s =
                    Vec3::from_array(from_scale_arr).lerp(Vec3::from_array(to_scale_arr), blend_t);
                ([q.x, q.y, q.z, q.w], [t.x, t.y, t.z], [s.x, s.y, s.z])
            }
            None => (to_rot, to_trans, to_scale_arr),
        };
        out[i] = Some(bone::BoneLocal {
            rotation: rot,
            translation: trans,
            scale,
        });
    }
    out
}

fn override_prefix(name: &str) -> [u8; 3] {
    let bytes = name.as_bytes();
    let mut p = [0u8; 3];
    for (i, slot) in p.iter_mut().enumerate() {
        *slot = bytes.get(i).copied().unwrap_or(0).to_ascii_lowercase();
    }
    p
}

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

    let baked = baked_for_mesh(&loaded.mesh, baked_owned);

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

                reflectance: 0.0,
                cull_mode: None,
                ..default()
            });

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

        spawn_one(
            commands,
            meshes,
            materials,
            positions.clone(),
            normals.clone(),
            uvs.clone(),
            indices.clone(),
        );

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

pub struct LoadedSlot {
    pub loaded: LoadedVos2,
    pub label: String,
}

pub struct PreparedEquipped {
    pub slots: Vec<LoadedSlot>,
    pub feet_translation_y: f32,
    pub min_mesh_y: f32,
    pub max_mesh_y: f32,
    pub race: u8,
}

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
        let _ = &slot.label;
    }

    if spawned > 0 {
        let actor_height = (prepared.max_mesh_y - prepared.min_mesh_y).max(0.1);

        commands.entity(parent).try_insert(BakedActor {
            min_mesh_y: prepared.min_mesh_y,
            actor_height,
        });
    }
    spawned
}

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

fn pbr_from_specular(exponent: f32, _intensity: f32) -> (f32, f32) {
    let roughness = if exponent <= 0.0 {
        1.0
    } else {
        (1.0 - (exponent.ln_1p() / 5.0)).clamp(0.3, 1.0)
    };
    (roughness, 0.0)
}

#[cfg(test)]
mod ffxi_skin_tests {
    use super::*;
    use ffxi_dat::vos2::{Vos2BoneIndices, Vos2Header, Vos2Vertex};

    fn empty_header(flip: u16, kind_type: u16) -> Vos2Header {
        Vos2Header {
            version: 1,
            kind_type,
            flip,
            off_poly_bytes: 0,
            off_bone_table_bytes: 0,
            bone_table_count: 0,
            off_weight_bytes: 0,
            off_bone_bytes: 0,
            bone_indices_count: 0,
            off_vertex_bytes: 0,
            off_poly_load_bytes: 0,
            poly_lod2_count: 0,
        }
    }

    #[test]
    fn rigid_and_mirror_expansion() {
        let mesh = Vos2Mesh {
            header: empty_header(1, 0),
            vertices: vec![
                Vos2Vertex {
                    pos: [1.0, 2.0, 3.0],
                    normal: [1.0, 0.0, 0.0],
                },
                Vos2Vertex {
                    pos: [4.0, 5.0, 6.0],
                    normal: [0.0, 1.0, 0.0],
                },
            ],
            groups: vec![],
            bone_table: vec![],

            bone_indices: vec![
                Vos2BoneIndices {
                    bone_index1: 3,
                    bone_index2: 5,
                    mirror_axis: 1,
                },
                Vos2BoneIndices {
                    bone_index1: 0,
                    bone_index2: 0,
                    mirror_axis: 0,
                },
                Vos2BoneIndices {
                    bone_index1: 4,
                    bone_index2: 6,
                    mirror_axis: 1,
                },
                Vos2BoneIndices {
                    bone_index1: 0,
                    bone_index2: 0,
                    mirror_axis: 0,
                },
            ],
            bone_weights: vec![],
        };

        let (vd, mirrored) = build_ffxi_vertex_data(&mesh, 16);
        assert!(mirrored);
        assert_eq!(vd.position0.len(), 4);

        assert_eq!(vd.joint0[0], 3);
        assert_eq!(vd.joint1[0], 3);
        assert_eq!(vd.weight[0], 1.0);
        assert_eq!(vd.position0[0], [1.0, 2.0, 3.0]);
        assert_eq!(vd.position1[0], [0.0, 0.0, 0.0]);

        assert_eq!(vd.joint0[2], 5);
        assert_eq!(vd.position0[2], [-1.0, 2.0, 3.0]);
        assert_eq!(vd.normal0[2], [-1.0, 0.0, 0.0]);

        assert_eq!(vd.joint0[3], 6);
        assert_eq!(vd.position0[3], [-4.0, 5.0, 6.0]);
    }

    #[test]
    fn overflow_bone_clamps_to_zero() {
        let mesh = Vos2Mesh {
            header: empty_header(0, 0),
            vertices: vec![Vos2Vertex {
                pos: [0.0; 3],
                normal: [0.0; 3],
            }],
            groups: vec![],
            bone_table: vec![],
            bone_indices: vec![
                Vos2BoneIndices {
                    bone_index1: 99,
                    bone_index2: 0,
                    mirror_axis: 0,
                },
                Vos2BoneIndices {
                    bone_index1: 0,
                    bone_index2: 0,
                    mirror_axis: 0,
                },
            ],
            bone_weights: vec![],
        };
        let (vd, mirrored) = build_ffxi_vertex_data(&mesh, 16);
        assert!(!mirrored);
        assert_eq!(vd.joint0[0], 0);
    }
}
