#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};

use ffxi_actor::actor_state::{self, ActorAnimInputs, RestKind};
use ffxi_actor::animation::{LoopParams, SkeletonAnimationCoordinator, TransitionParams};
use ffxi_actor::skeleton_instance::{
    apply_head_look, find_head_neck, neck_subtree, pose_world, RootTransform,
};

use ffxi_dat::d3m::D3m;
use ffxi_dat::datid::DatId;
use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::scheduler::{Scheduler, StageKind};
use ffxi_dat::skel::Skeleton;
use ffxi_dat::skel_anim::SkeletonAnimation;
use ffxi_dat::skel_mesh::{MeshBuffer, MeshType, SkelMesh};
use ffxi_dat::texture::{decode_texture, DecodedTexture};
use ffxi_dat::{walk_tree, ChunkKind, ChunkNode, DatRoot};

use crate::combat_stance;
use crate::dat_vos2::skeleton_file_id_for_race;
use crate::skinned_ffxi_material::{
    FfxiJointMatrices, FfxiLightingUniform, FfxiMaterialFlags, FfxiSkinnedMaterial, ATTR_COLOR,
    ATTR_JOINT0, ATTR_JOINT1, ATTR_JOINT_WEIGHT, ATTR_NORMAL0, ATTR_NORMAL1, ATTR_POSITION0,
    ATTR_POSITION1,
};

#[derive(Debug, Clone)]
pub enum ActorSubject {
    Pc {
        race: u8,
        equipment: Vec<u32>,
        main_weapon: Option<u32>,
        sub_weapon: Option<u32>,
    },

    Npc {
        file_id: u32,
    },
}

#[derive(Message, Debug, Clone)]
pub struct LoadActorRequest {
    pub entity_id: u32,
    pub subject: ActorSubject,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct FfxiRenderRoot(pub Entity);

pub const FRAME_RATE: f32 = 30.0;

pub const LOCOMOTION_XFADE_IN: f32 = 9.0;

pub const LOCOMOTION_XFADE_OUT: f32 = 7.5;

pub const WALK_RUN_BOUNDARY: f32 = 3.0;

#[inline]
pub fn infers_walk_gait(speed: f32) -> bool {
    speed > combat_stance::EntityMotion::MOVE_EXIT && speed < WALK_RUN_BOUNDARY
}

fn ffxi_to_bevy_basis() -> Quat {
    Quat::from_rotation_x(std::f32::consts::PI)
}

struct NamedTexture {
    name: String,
    texture: DecodedTexture,
}

pub struct LoadedActor {
    pub skeleton: Arc<Skeleton>,

    pub skel_meshes: Vec<SkelMesh>,

    effect_meshes: Vec<D3m>,

    textures: Vec<NamedTexture>,

    animations: Arc<Vec<SkeletonAnimation>>,

    battle_clips: Arc<Vec<SkeletonAnimation>>,

    routines: Arc<HashMap<DatId, Scheduler>>,
}

// Clip/scheduler parsing is the expensive tail of an actor load; deriving it here
// keeps it on the loader task instead of the render main thread, and the Arcs let
// consumers share the parsed sets without deep-cloning keyframe data.
fn derive_animation_sets(
    anim_dirs: &[ResourceDir],
    battle_dirs: &[ResourceDir],
) -> (
    Arc<Vec<SkeletonAnimation>>,
    Arc<Vec<SkeletonAnimation>>,
    Arc<HashMap<DatId, Scheduler>>,
) {
    let animations = dedup_clips(anim_dirs.iter());
    let battle_clips = dedup_clips(battle_dirs.iter());
    let mut routines: HashMap<DatId, Scheduler> = HashMap::new();
    for dir in battle_dirs.iter().chain(anim_dirs.iter()) {
        for sched in dir.collect_schedulers() {
            routines
                .entry(DatId::from_name(&sched.name))
                .or_insert(sched);
        }
    }
    (
        Arc::new(animations),
        Arc::new(battle_clips),
        Arc::new(routines),
    )
}

// Everything CPU-heavy about turning a LoadedActor into spawnable pieces —
// vertex conversion, mip-chain generation, bind pose — happens here so the
// loader task pays it, not the render main thread.
pub struct PreparedParts {
    images: Vec<Image>,

    skel_built: Vec<BuiltGroup>,

    d3m_built: Vec<BuiltGroup>,

    bind_joints: FfxiJointMatrices,
}

pub struct PreparedActor {
    pub loaded: LoadedActor,
    parts: PreparedParts,
}

fn prepare_actor_parts(
    loaded: &LoadedActor,
    facing_dir: f32,
    scale: f32,
    q: crate::zone_texture::TextureQuality,
) -> PreparedParts {
    let occlusion: std::collections::HashSet<u8> =
        loaded.skel_meshes.iter().map(|m| m.occlude_type).collect();
    let joint_count = loaded.skeleton.joints.len();

    let mut bind_joints = FfxiJointMatrices::default();
    bind_joints.set_from(&pose_world(
        &loaded.skeleton,
        |_| None,
        RootTransform {
            facing_dir,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::splat(scale),
        },
        &[],
    ));

    let mut skel_built = Vec::new();
    for skel_mesh in &loaded.skel_meshes {
        for buffer in &skel_mesh.meshes {
            if buffer.vertices.is_empty() || is_occluded(buffer, &occlusion) {
                continue;
            }
            skel_built.push(BuiltGroup {
                mesh: build_mesh(buffer, joint_count),
                texture_name: buffer.texture_name.clone(),
            });
        }
    }

    let mut d3m_built = Vec::new();
    for d3m in &loaded.effect_meshes {
        if d3m.vertices.is_empty() {
            continue;
        }
        d3m_built.push(BuiltGroup {
            mesh: build_d3m_mesh(d3m),
            texture_name: d3m.texture_name_str(),
        });
    }

    let images = loaded
        .textures
        .iter()
        .map(|nt| decoded_texture_to_image(&nt.texture, q))
        .collect();

    PreparedParts {
        images,
        skel_built,
        d3m_built,
        bind_joints,
    }
}

// Re-sightings are constant while moving (entities flap in/out of the server's
// sight radius), so prepared actors are cached by look + texture quality.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ActorPrepKey {
    Npc {
        file_id: u32,
        mipmaps: bool,
        anisotropy: u16,
    },
    Pc {
        race: u8,
        equipment: Vec<u32>,
        main_weapon: Option<u32>,
        sub_weapon: Option<u32>,
        mipmaps: bool,
        anisotropy: u16,
    },
}

fn prep_key(subject: &ActorSubject, q: crate::zone_texture::TextureQuality) -> ActorPrepKey {
    match subject {
        ActorSubject::Npc { file_id } => ActorPrepKey::Npc {
            file_id: *file_id,
            mipmaps: q.mipmaps,
            anisotropy: q.anisotropy,
        },
        ActorSubject::Pc {
            race,
            equipment,
            main_weapon,
            sub_weapon,
        } => ActorPrepKey::Pc {
            race: *race,
            equipment: equipment.clone(),
            main_weapon: *main_weapon,
            sub_weapon: *sub_weapon,
            mipmaps: q.mipmaps,
            anisotropy: q.anisotropy,
        },
    }
}

const ACTOR_PREP_CACHE_CAP: usize = 48;

#[derive(Default)]
struct ActorPrepCache {
    map: HashMap<ActorPrepKey, Arc<PreparedActor>>,
    order: std::collections::VecDeque<ActorPrepKey>,
}

impl ActorPrepCache {
    fn get_and_promote(&mut self, key: &ActorPrepKey) -> Option<Arc<PreparedActor>> {
        let hit = self.map.get(key).cloned()?;
        self.order.retain(|k| k != key);
        self.order.push_back(key.clone());
        Some(hit)
    }

    fn insert(&mut self, key: ActorPrepKey, prepared: Arc<PreparedActor>) {
        if self.map.insert(key.clone(), prepared).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > ACTOR_PREP_CACHE_CAP {
            let Some(evict) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&evict);
        }
    }
}

fn read_dat(root: &DatRoot, file_id: u32) -> Option<Vec<u8>> {
    let loc = root.resolve(file_id).ok()?;
    fs::read(loc.path_under(root.root())).ok()
}

fn dedup_clips<'a>(dirs: impl Iterator<Item = &'a ResourceDir>) -> Vec<SkeletonAnimation> {
    let mut out: Vec<SkeletonAnimation> = Vec::new();
    let mut seen: std::collections::HashSet<DatId> = std::collections::HashSet::new();
    for dir in dirs {
        for anim in dir.collect_animations() {
            if seen.insert(anim.id) {
                out.push(anim);
            }
        }
    }
    out
}

fn full_texture_name(body: &[u8]) -> String {
    body.get(1..0x11)
        .map(|raw| raw.iter().map(|&b| b as char).collect())
        .unwrap_or_default()
}

fn collect_textures(node: &ChunkNode<'_>, out: &mut Vec<NamedTexture>) {
    if ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::Img) {
        if let Ok(texture) = decode_texture(node.chunk.data) {
            let name = full_texture_name(node.chunk.data);
            out.push(NamedTexture { name, texture });
        }
    }
    for child in &node.children {
        collect_textures(child, out);
    }
}

fn collect_d3m(node: &ChunkNode<'_>, out: &mut Vec<D3m>) {
    if ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::D3m) {
        if let Ok(d) = D3m::parse(node.chunk.name, node.chunk.data) {
            if d.num_triangles > 2 {
                out.push(d);
            }
        }
    }
    for child in &node.children {
        collect_d3m(child, out);
    }
}

fn first_skeleton(bytes: &[u8]) -> Option<Skeleton> {
    ResourceDir::from_bytes(bytes.to_vec())
        .collect_skeletons()
        .into_iter()
        .next()
}

pub fn load_npc(file_id: u32) -> Result<LoadedActor, String> {
    crate::perf_probe::note_model_load();
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let bytes = read_dat(&root, file_id).ok_or_else(|| format!("read npc dat {file_id}"))?;

    let skeleton =
        first_skeleton(&bytes).ok_or_else(|| format!("no skeleton (0x29) in npc dat {file_id}"))?;

    let dir = ResourceDir::from_bytes(bytes.clone());
    let skel_meshes = dir.collect_skel_meshes();
    if skel_meshes.is_empty() {
        return Err(format!("no skeleton meshes (0x2A) in npc dat {file_id}"));
    }

    let tree = walk_tree(&bytes);
    let mut textures = Vec::new();
    collect_textures(&tree, &mut textures);
    let mut effect_meshes = Vec::new();
    collect_d3m(&tree, &mut effect_meshes);

    let anim_dirs = vec![ResourceDir::from_bytes(bytes)];
    let (animations, battle_clips, routines) = derive_animation_sets(&anim_dirs, &[]);
    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,
        effect_meshes,
        textures,
        animations,
        battle_clips,
        routines,
    })
}

fn default_pc_equipment(race: u8) -> Vec<u32> {
    use crate::look_resolver::{resolve_equipment_slot, resolve_face};
    let mut out = Vec::new();
    if let Some(f) = resolve_face(0, race) {
        out.push(f);
    }

    for slot in 1u16..=5 {
        if let Some(f) = resolve_equipment_slot(slot << 12, race) {
            out.push(f);
        }
    }
    out
}

/// Retail's PC race byte is only ever 1..=8 (`skeleton_file_id_for_race`'s
/// table is exactly that size — there is no retail skeleton for anything
/// else). An "Equipped"-look NPC broadcasting a race outside that range is
/// bad server-side data, not a client gap; falling back to Hume Male here
/// renders *a* humanoid instead of leaving the entity as a bare placeholder
/// orb, at the cost of a wrong race/equipment fit for that one NPC.
const FALLBACK_RACE: u8 = 1;

pub fn load_pc(
    race: u8,
    equipment: &[u32],
    main_weapon: Option<u32>,

    sub_weapon: Option<u32>,
) -> Result<LoadedActor, String> {
    crate::perf_probe::note_model_load();
    let _ = sub_weapon;
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let race = if skeleton_file_id_for_race(race).is_some() {
        race
    } else {
        warn!(
            race,
            fallback = FALLBACK_RACE,
            "load_pc: race has no retail skeleton, falling back"
        );
        FALLBACK_RACE
    };
    let skel_file_id =
        skeleton_file_id_for_race(race).ok_or_else(|| format!("unsupported race {race}"))?;

    let skel_bytes =
        read_dat(&root, skel_file_id).ok_or_else(|| format!("read skel dat {skel_file_id}"))?;
    let skeleton = first_skeleton(&skel_bytes)
        .ok_or_else(|| format!("no skeleton in race dat {skel_file_id}"))?;

    let mut skel_meshes = Vec::new();
    let mut textures = Vec::new();
    let mut anim_dirs = vec![ResourceDir::from_bytes(skel_bytes.clone())];

    {
        let dir = ResourceDir::from_bytes(skel_bytes.clone());
        skel_meshes.extend(dir.collect_skel_meshes());
        collect_textures(&walk_tree(&skel_bytes), &mut textures);
    }

    if let Some(bytes) = read_dat(&root, skel_file_id + 1) {
        anim_dirs.push(ResourceDir::from_bytes(bytes));
    }

    let resolved_default;
    let equipment = if equipment.is_empty() {
        resolved_default = default_pc_equipment(race);
        resolved_default.as_slice()
    } else {
        equipment
    };

    let mut equip_trace: Vec<(u32, &'static str)> = Vec::new();
    for &file_id in equipment {
        let Some(bytes) = read_dat(&root, file_id) else {
            equip_trace.push((file_id, "unreadable"));
            continue;
        };
        let dir = ResourceDir::from_bytes(bytes.clone());
        let meshes = dir.collect_skel_meshes();
        if meshes.is_empty() {
            equip_trace.push((file_id, "0 meshes"));
            continue;
        }
        equip_trace.push((file_id, "ok"));
        skel_meshes.extend(meshes);
        collect_textures(&walk_tree(&bytes), &mut textures);
        anim_dirs.push(ResourceDir::from_bytes(bytes));
    }
    debug!("load_pc race={race}: equipment {equip_trace:?}");
    // A slot that resolved to a file but yielded no mesh renders as a missing
    // body part (e.g. a headless PC when a head model fails to load) — surface it
    // instead of silently dropping at debug level.
    let dropped: Vec<u32> = equip_trace
        .iter()
        .filter(|(_, status)| *status != "ok")
        .map(|&(file_id, _)| file_id)
        .collect();
    if !dropped.is_empty() {
        warn!("load_pc race={race}: equipment files resolved but unrendered {dropped:?}");
    }

    let weapon_anim_type = main_weapon
        .and_then(|wf| read_dat(&root, wf))
        .map(ResourceDir::from_bytes)
        .and_then(|d| d.first_cib())
        .map(|c| c.motion_index)
        .unwrap_or(0);
    let mut battle_dirs = Vec::new();
    if let Some(base) = combat_stance::motion_dat_for_skel(skel_file_id) {
        if weapon_anim_type != 0 && weapon_anim_type != 0xFF {
            if let Some(dir) = read_dat(&root, base + weapon_anim_type as u32)
                .map(ResourceDir::from_bytes)
                .filter(|d| {
                    d.collect_animations()
                        .iter()
                        .any(|a| a.id.as_str().starts_with("btl"))
                })
            {
                battle_dirs.push(dir);
            }
        }

        if let Some(dir) = read_dat(&root, base).map(ResourceDir::from_bytes) {
            battle_dirs.push(dir);
        }
    }
    if battle_dirs.is_empty() {
        warn!("load_pc race={race}: no battle dir resolved — stance/swings unavailable");
    }

    if skel_meshes.is_empty() {
        return Err(format!(
            "no skeleton meshes for race {race} equipment {equipment:?}"
        ));
    }

    let (animations, battle_clips, routines) = derive_animation_sets(&anim_dirs, &battle_dirs);
    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,

        effect_meshes: Vec::new(),
        textures,
        animations,
        battle_clips,
        routines,
    })
}

fn is_occluded(buffer: &MeshBuffer, occlusion: &std::collections::HashSet<u8>) -> bool {
    let has = |v: u8| occlusion.contains(&v);
    match buffer.render_properties.display_type_flag {
        0 => false,

        1 => has(0x02) || has(0x03) || has(0x04) || has(0x05) || has(0x06),

        2 | 3 => has(0x04) || has(0x05) || has(0x06),

        4 => has(0x05),

        5 => has(0x12),

        6 => has(0x32),

        7 => has(0x22),

        _ => false,
    }
}

struct BuiltGroup {
    mesh: Mesh,
    texture_name: String,
}

fn clamp_joint(idx: u16, joint_count: usize) -> u32 {
    let i = idx as usize;
    if i < joint_count {
        i as u32
    } else {
        0
    }
}

fn build_mesh(buffer: &MeshBuffer, joint_count: usize) -> Mesh {
    let n = buffer.vertices.len();

    let mut position0 = Vec::with_capacity(n);
    let mut position1 = Vec::with_capacity(n);
    let mut normal0 = Vec::with_capacity(n);
    let mut normal1 = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut weight = Vec::with_capacity(n);
    let mut joint0 = Vec::with_capacity(n);
    let mut joint1 = Vec::with_capacity(n);
    let mut color = Vec::with_capacity(n);

    for v in &buffer.vertices {
        position0.push(v.p0);
        position1.push(v.p1);
        normal0.push(v.n0);
        normal1.push(v.n1);
        uvs.push([v.u, v.v]);

        weight.push(v.joint0_weight);
        joint0.push(clamp_joint(v.joint_index0, joint_count));
        joint1.push(clamp_joint(v.joint_index1, joint_count));
        color.push([
            v.color[0] as f32 / 255.0,
            v.color[1] as f32 / 255.0,
            v.color[2] as f32 / 255.0,
            v.color[3] as f32 / 255.0,
        ]);
    }

    let topology = match buffer.mesh_type {
        MeshType::Strip => PrimitiveTopology::TriangleStrip,
        MeshType::Mesh => PrimitiveTopology::TriangleList,
    };

    let mut mesh = Mesh::new(topology, RenderAssetUsages::default());
    mesh.insert_attribute(ATTR_POSITION0, position0);
    mesh.insert_attribute(ATTR_POSITION1, position1);
    mesh.insert_attribute(ATTR_NORMAL0, normal0);
    mesh.insert_attribute(ATTR_NORMAL1, normal1);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(ATTR_JOINT_WEIGHT, weight);
    mesh.insert_attribute(ATTR_JOINT0, VertexAttributeValues::Uint32(joint0));
    mesh.insert_attribute(ATTR_JOINT1, VertexAttributeValues::Uint32(joint1));
    mesh.insert_attribute(ATTR_COLOR, color);
    mesh.insert_indices(Indices::U32((0..n as u32).collect()));
    mesh
}

fn build_d3m_mesh(d3m: &D3m) -> Mesh {
    let n = d3m.vertices.len();
    let mut position0 = Vec::with_capacity(n);
    let mut position1 = Vec::with_capacity(n);
    let mut normal0 = Vec::with_capacity(n);
    let mut normal1 = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut weight = Vec::with_capacity(n);
    let mut joint0 = Vec::with_capacity(n);
    let mut joint1 = Vec::with_capacity(n);
    let mut color = Vec::with_capacity(n);

    for v in &d3m.vertices {
        position0.push(v.pos);
        position1.push([0.0, 0.0, 0.0]);
        normal0.push(v.normal);
        normal1.push([0.0, 0.0, 0.0]);
        uvs.push(v.uv);
        weight.push(1.0);
        joint0.push(0u32);
        joint1.push(0u32);
        color.push([
            v.color[0].clamp(0.0, 1.0),
            v.color[1].clamp(0.0, 1.0),
            v.color[2].clamp(0.0, 1.0),
            v.color[3].clamp(0.0, 1.0),
        ]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(ATTR_POSITION0, position0);
    mesh.insert_attribute(ATTR_POSITION1, position1);
    mesh.insert_attribute(ATTR_NORMAL0, normal0);
    mesh.insert_attribute(ATTR_NORMAL1, normal1);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(ATTR_JOINT_WEIGHT, weight);
    mesh.insert_attribute(ATTR_JOINT0, VertexAttributeValues::Uint32(joint0));
    mesh.insert_attribute(ATTR_JOINT1, VertexAttributeValues::Uint32(joint1));
    mesh.insert_attribute(ATTR_COLOR, color);
    mesh.insert_indices(Indices::U32((0..n as u32).collect()));
    mesh
}

struct TextureKey {
    name_space: String,
    local_name: String,
}

impl TextureKey {
    fn from_full(name: &str) -> Self {
        let trim = |s: &str| s.trim_end_matches(['\0', ' ']).to_string();
        if name.len() >= 16 {
            TextureKey {
                name_space: trim(&name[0..8]),
                local_name: trim(&name[8..16]),
            }
        } else {
            TextureKey {
                name_space: String::new(),
                local_name: trim(name),
            }
        }
    }

    fn full_key(&self) -> String {
        format!("{}/{}", self.name_space, self.local_name)
    }
}

fn is_blank_texture(name: &str) -> bool {
    name.trim_matches(['\0', ' ']).is_empty()
}

#[derive(Component)]
pub struct FfxiRenderActor {
    pub skeleton: Arc<Skeleton>,

    animations: Arc<Vec<SkeletonAnimation>>,

    battle_clips: Arc<Vec<SkeletonAnimation>>,

    routines: Arc<HashMap<DatId, Scheduler>>,
    coordinator: SkeletonAnimationCoordinator,
    materials: Vec<Handle<FfxiSkinnedMaterial>>,

    pub inputs: ActorAnimInputs,

    pub world_id: u32,

    pub facing_dir: f32,

    pub scale: f32,

    current_clip: Option<(DatId, bool)>,

    rest_phase: RestPlayback,

    engage: EngageMachine,

    action: Option<ActionPlayback>,
    action_clips: Vec<SkeletonAnimation>,

    head_neck: Option<usize>,
    head_subtree: Vec<usize>,

    head_rot: Quat,

    pub last_clip: Option<DatId>,
    pub last_frame: f32,
}

impl FfxiRenderActor {
    pub fn material_handles(&self) -> &[Handle<FfxiSkinnedMaterial>] {
        &self.materials
    }

    pub fn begin_completion_motion(&mut self, clip_id: DatId, motion: CompletionMotion) {
        // research/xim EffectRoutineInterpolatedEffects.kt:49 — a skill's body motion is
        // resolved against `listOf(localDir) + actor.getAllAnimationDirectories()`: the
        // skill DAT's own clips first, then the caster's. Stash the matching local clips so
        // select_pose_clips_layered finds them ahead of the actor's own pose set.
        self.action_clips = motion
            .local_clips
            .iter()
            .filter(|a| a.id.parameterized_match(&clip_id))
            .cloned()
            .collect();

        let len = rest_clip_len_frames(&self.action_clips, clip_id)
            .max(rest_clip_len_frames(&self.battle_clips, clip_id))
            .max(rest_clip_len_frames(&self.animations, clip_id));
        // research/xim EffectRoutineInterpolatedEffects.kt:50-51 — half-frame fields become
        // real frames at rate 1.0 by halving; maxLoops>1 means the motion repeats.
        let num_loops = (motion.max_loops > 1).then_some(motion.max_loops as u32);
        self.action = Some(ActionPlayback {
            clip_id,
            looping: num_loops.is_some(),
            remaining: len.max(motion.duration_frames * 0.5).max(1.0),
            num_loops,
            transition_in: half_frames(motion.transition_in),
            transition_out: half_frames(motion.transition_out),
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum EngageMachine {
    NotEngaged,

    Drawing { remaining: f32 },
    Engaged,

    Sheathing { remaining: f32 },
}

pub struct CompletionMotion<'a> {
    pub local_clips: &'a [SkeletonAnimation],
    pub duration_frames: f32,
    pub max_loops: u16,
    pub transition_in: u16,
    pub transition_out: u16,
}

fn half_frames(v: u16) -> f32 {
    v as f32 * 0.5
}

#[derive(Clone, Copy)]
struct ActionPlayback {
    clip_id: DatId,

    looping: bool,

    remaining: f32,

    num_loops: Option<u32>,
    transition_in: f32,
    transition_out: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum RestPlayback {
    Inactive,

    Starting { kind: RestKind, remaining: f32 },

    Looping { kind: RestKind },

    Stopping { kind: RestKind, remaining: f32 },
}

impl LoadedActor {
    fn all_animations(&self) -> Arc<Vec<SkeletonAnimation>> {
        Arc::clone(&self.animations)
    }

    fn all_battle_clips(&self) -> Arc<Vec<SkeletonAnimation>> {
        Arc::clone(&self.battle_clips)
    }

    fn all_routines(&self) -> Arc<HashMap<DatId, Scheduler>> {
        Arc::clone(&self.routines)
    }

    pub fn bind_pose_bounds(&self, facing_dir: f32, scale: f32) -> Option<(Vec3, Vec3)> {
        let pose = pose_world(
            &self.skeleton,
            |_| None,
            RootTransform {
                facing_dir,
                skew: 0.0,
                slope_oriented: false,
                scale: Vec3::splat(scale),
            },
            &[],
        );
        let basis = ffxi_to_bevy_basis();
        let joint_count = self.skeleton.joints.len();
        let occlusion: std::collections::HashSet<u8> =
            self.skel_meshes.iter().map(|m| m.occlude_type).collect();

        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        let mut any = false;
        for skel_mesh in &self.skel_meshes {
            for buffer in &skel_mesh.meshes {
                if buffer.vertices.is_empty() || is_occluded(buffer, &occlusion) {
                    continue;
                }
                for v in &buffer.vertices {
                    let w = v.joint0_weight;
                    let j0 = clamp_joint(v.joint_index0, joint_count) as usize;
                    let j1 = clamp_joint(v.joint_index1, joint_count) as usize;
                    let m0 = pose.get(j0).copied().unwrap_or(Mat4::IDENTITY);
                    let m1 = pose.get(j1).copied().unwrap_or(Mat4::IDENTITY);
                    let p = m0 * Vec4::new(v.p0[0], v.p0[1], v.p0[2], w)
                        + m1 * Vec4::new(v.p1[0], v.p1[1], v.p1[2], 1.0 - w);
                    let wp = basis * p.truncate();
                    lo = lo.min(wp);
                    hi = hi.max(wp);
                    any = true;
                }
            }
        }

        let root = pose.first().copied().unwrap_or(Mat4::IDENTITY);
        for d3m in &self.effect_meshes {
            for v in &d3m.vertices {
                let p = root * Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
                let wp = basis * p.truncate();
                lo = lo.min(wp);
                hi = hi.max(wp);
                any = true;
            }
        }
        any.then_some((lo, hi))
    }
}

pub fn spawn_loaded_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    world_pos: Vec3,
    facing_dir: f32,
    scale: f32,
    q: crate::zone_texture::TextureQuality,
) -> Entity {
    let actor_root = commands
        .spawn((
            Transform {
                translation: world_pos,
                rotation: ffxi_to_bevy_basis(),
                scale: Vec3::ONE,
            },
            GlobalTransform::default(),
            Visibility::default(),
        ))
        .id();

    let parts = prepare_actor_parts(loaded, facing_dir, scale, q);
    let material_handles = build_actor_children(
        commands, meshes, materials, images, loaded, &parts, actor_root,
    );

    commands.entity(actor_root).insert(make_render_actor(
        loaded,
        material_handles,
        0,
        facing_dir,
        scale,
    ));

    actor_root
}

#[allow(clippy::too_many_arguments)]
#[derive(Component)]
pub(crate) struct FfxiActorMeshChild;

fn build_actor_children(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    parts: &PreparedParts,
    actor_root: Entity,
) -> Vec<Handle<FfxiSkinnedMaterial>> {
    let mut by_full: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(loaded.textures.len());
    let mut by_local: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(loaded.textures.len());
    let mut by_trimmed: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(loaded.textures.len());
    for (nt, image) in loaded.textures.iter().zip(parts.images.iter()) {
        let handle = images.add(image.clone());
        let trimmed = nt.name.trim_end_matches(['\0', ' ']).to_string();
        if !trimmed.is_empty() {
            by_trimmed.entry(trimmed).or_insert(handle.clone());
        }
        let key = TextureKey::from_full(&nt.name);
        if key.local_name.is_empty() {
            continue;
        }
        by_full.entry(key.full_key()).or_insert(handle.clone());
        by_local.entry(key.local_name).or_insert(handle);
    }
    let resolve_texture = |name: &str| -> Option<Handle<Image>> {
        let key = TextureKey::from_full(name);
        by_full
            .get(&key.full_key())
            .or_else(|| by_local.get(&key.local_name))
            .or_else(|| by_trimmed.get(name.trim_end_matches(['\0', ' '])))
            .cloned()
    };

    let mut material_handles = Vec::new();

    for built in parts.skel_built.iter().chain(parts.d3m_built.iter()) {
        let untextured = is_blank_texture(&built.texture_name);
        let tex_handle = if untextured {
            None
        } else {
            resolve_texture(&built.texture_name)
        };
        let has_texture = if tex_handle.is_some() { 1.0 } else { 0.0 };

        let mat = materials.add(FfxiSkinnedMaterial::new(
            actor_root.to_bits(),
            FfxiLightingUniform::default(),
            tex_handle,
            parts.bind_joints.clone(),
            FfxiMaterialFlags {
                flags: Vec4::new(has_texture, 0.0, 0.0, 0.0),
            },
        ));
        material_handles.push(mat.clone());

        commands.spawn((
            Mesh3d(meshes.add(built.mesh.clone())),
            MeshMaterial3d(mat),
            Transform::default(),
            FfxiActorMeshChild,
            ChildOf(actor_root),
        ));
    }

    material_handles
}

// Not gated on resource_changed: it must run every frame so actor meshes spawned
// after a settings change still get the current value via the Added query (full
// sweeps over q_all happen only when the setting itself changes).
pub(crate) fn apply_character_shadow_cast(
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut commands: Commands,
    q_added: Query<Entity, Added<FfxiActorMeshChild>>,
    q_all: Query<Entity, With<FfxiActorMeshChild>>,
) {
    let cast = settings.character_shadow_cast;
    let mut apply = |e: Entity| {
        let mut ec = commands.entity(e);
        if cast {
            ec.remove::<bevy::light::NotShadowCaster>();
        } else {
            ec.insert(bevy::light::NotShadowCaster);
        }
    };
    if settings.is_changed() {
        for e in &q_all {
            apply(e);
        }
    } else {
        for e in &q_added {
            apply(e);
        }
    }
}

fn make_render_actor(
    loaded: &LoadedActor,
    materials: Vec<Handle<FfxiSkinnedMaterial>>,
    world_id: u32,
    facing_dir: f32,
    scale: f32,
) -> FfxiRenderActor {
    let (head_neck, head_subtree) = match find_head_neck(&loaded.skeleton) {
        Some((neck, _head)) => (Some(neck), neck_subtree(&loaded.skeleton, neck)),
        None => (None, Vec::new()),
    };
    FfxiRenderActor {
        skeleton: loaded.skeleton.clone(),
        animations: loaded.all_animations(),
        battle_clips: loaded.all_battle_clips(),
        routines: loaded.all_routines(),
        coordinator: SkeletonAnimationCoordinator::new(),
        materials,
        inputs: ActorAnimInputs::default(),
        world_id,
        facing_dir,
        scale,
        current_clip: None,
        rest_phase: RestPlayback::Inactive,
        engage: EngageMachine::NotEngaged,
        action: None,
        action_clips: Vec::new(),
        head_neck,
        head_subtree,
        head_rot: Quat::IDENTITY,
        last_clip: None,
        last_frame: 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_live_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    prepared: &PreparedActor,
    wire_entity: Entity,
    world_id: u32,
    scale: f32,
) -> Entity {
    let facing_dir = 0.0;

    let actor_root = commands
        .spawn((
            Transform {
                translation: Vec3::ZERO,
                rotation: ffxi_to_bevy_basis(),
                scale: Vec3::ONE,
            },
            GlobalTransform::default(),
            Visibility::default(),
            ChildOf(wire_entity),
        ))
        .id();

    let material_handles = build_actor_children(
        commands,
        meshes,
        materials,
        images,
        &prepared.loaded,
        &prepared.parts,
        actor_root,
    );

    commands.entity(actor_root).insert(make_render_actor(
        &prepared.loaded,
        material_handles,
        world_id,
        facing_dir,
        scale,
    ));

    actor_root
}

fn decoded_texture_to_image(t: &DecodedTexture, q: crate::zone_texture::TextureQuality) -> Image {
    // Mip chain + anisotropic sampler (the zone path's builder). Alpha is left
    // exactly as the decoder produced it — the actor path does not apply the zone
    // alpha remap — so only filtering changes here. Filtering follows the GUI
    // Texture Filtering setting, like the zone/MMB paths.
    crate::zone_texture::image_with_mips(
        t.rgba.clone(),
        t.width,
        t.height,
        q,
        crate::zone_texture::has_cutout_alpha(t),
    )
}

pub fn tick_ffxi_render_actors(
    time: Res<Time>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut q_actors: Query<&mut FfxiRenderActor>,
) {
    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    for mut actor in &mut q_actors {
        advance_actor_pose(&mut actor, elapsed_frames, &mut materials, None);
    }
}

fn select_pose_clips_layered(
    primary: &[SkeletonAnimation],
    overlay: &[SkeletonAnimation],
    selected_id: DatId,
) -> Vec<SkeletonAnimation> {
    let collect = |id: DatId| -> Vec<SkeletonAnimation> {
        let mut seen: std::collections::HashSet<DatId> = std::collections::HashSet::new();
        overlay
            .iter()
            .chain(primary.iter())
            .filter(|a| a.id.parameterized_match(&id) && seen.insert(a.id))
            .cloned()
            .collect()
    };
    let m = collect(selected_id);
    if m.is_empty() {
        collect(DatId::from_str("idl?"))
    } else {
        m
    }
}

fn rest_clip_len_frames(animations: &[SkeletonAnimation], id: DatId) -> f32 {
    animations
        .iter()
        .filter(|a| a.id.parameterized_match(&id))
        .map(|a| a.length_in_frames())
        .fold(0.0_f32, f32::max)
}

fn advance_rest_phase(
    phase: &mut RestPlayback,
    desired: RestKind,
    animations: &[SkeletonAnimation],
    elapsed_frames: f32,
) -> Option<DatId> {
    use actor_state::RestPhase;

    let begin_in = |phase: &mut RestPlayback, kind: RestKind| {
        let id = actor_state::rest_animation_id_phase(kind, RestPhase::In).unwrap();
        *phase = RestPlayback::Starting {
            kind,
            remaining: rest_clip_len_frames(animations, id),
        };
        Some(id)
    };

    let begin_out = |phase: &mut RestPlayback, kind: RestKind| {
        let id = actor_state::rest_animation_id_phase(kind, RestPhase::Out).unwrap();
        *phase = RestPlayback::Stopping {
            kind,
            remaining: rest_clip_len_frames(animations, id),
        };
        Some(id)
    };

    match *phase {
        RestPlayback::Inactive => {
            if desired == RestKind::None {
                None
            } else {
                begin_in(phase, desired)
            }
        }
        RestPlayback::Starting { kind, remaining } => {
            if desired == RestKind::None {
                begin_out(phase, kind)
            } else if desired != kind {
                begin_in(phase, desired)
            } else {
                let remaining = remaining - elapsed_frames;
                if remaining <= 0.0 {
                    *phase = RestPlayback::Looping { kind };
                    actor_state::rest_animation_id_phase(kind, RestPhase::Loop)
                } else {
                    *phase = RestPlayback::Starting { kind, remaining };
                    actor_state::rest_animation_id_phase(kind, RestPhase::In)
                }
            }
        }
        RestPlayback::Looping { kind } => {
            if desired == RestKind::None {
                begin_out(phase, kind)
            } else if desired != kind {
                begin_in(phase, desired)
            } else {
                actor_state::rest_animation_id_phase(kind, RestPhase::Loop)
            }
        }
        RestPlayback::Stopping { kind, remaining } => {
            if desired == kind {
                begin_in(phase, kind)
            } else {
                let remaining = remaining - elapsed_frames;
                if remaining <= 0.0 {
                    *phase = RestPlayback::Inactive;
                    None
                } else {
                    *phase = RestPlayback::Stopping { kind, remaining };
                    actor_state::rest_animation_id_phase(kind, RestPhase::Out)
                }
            }
        }
    }
}

fn routine_motion_clip(routines: &HashMap<DatId, Scheduler>, routine: DatId) -> Option<DatId> {
    let sched = routines.get(&routine)?;
    sched
        .stages
        .iter()
        .find(|t| t.stage.kind == StageKind::Motion)
        .map(|t| DatId::from_name(&t.stage.id))
}

fn action_routine(action_kind: u8, action_id: u32) -> Option<(DatId, bool)> {
    Some(match action_kind {
        1 => (DatId::from_str("ati0"), false),

        7 => (DatId::from_str("ati0"), false),

        8 => {
            let id = ffxi_proto::magic::cast_suffix(action_id)
                .map(|s| DatId::from_str(&format!("ca{s}")))
                .unwrap_or_else(|| DatId::from_str("cast"));
            (id, true)
        }

        9 => (DatId::from_str("cait"), false),

        10 => (DatId::from_str("cast"), true),

        12 => (DatId::from_str("calg"), true),
        _ => return None,
    })
}

fn advance_engage(
    machine: &mut EngageMachine,
    want_engaged: bool,
    routines: &HashMap<DatId, Scheduler>,
    animations: &[SkeletonAnimation],
    elapsed_frames: f32,
) -> actor_state::EngageAnimationState {
    use actor_state::EngageAnimationState as S;

    let transition_len = |routine: &str| -> f32 {
        routine_motion_clip(routines, DatId::from_str(routine))
            .map(|clip| rest_clip_len_frames(animations, clip))
            .unwrap_or(0.0)
    };

    match *machine {
        EngageMachine::NotEngaged => {
            if !want_engaged {
                return S::NotEngaged;
            }
            let len = transition_len("in 0");
            if len > 0.0 {
                *machine = EngageMachine::Drawing { remaining: len };
                S::Engaging
            } else {
                *machine = EngageMachine::Engaged;
                S::Engaged
            }
        }
        EngageMachine::Drawing { remaining } => {
            if !want_engaged {
                let len = transition_len("out0");
                if len > 0.0 {
                    *machine = EngageMachine::Sheathing { remaining: len };
                    return S::Disengaging;
                }
                *machine = EngageMachine::NotEngaged;
                return S::NotEngaged;
            }
            let remaining = remaining - elapsed_frames;
            if remaining <= 0.0 {
                *machine = EngageMachine::Engaged;
                S::Engaged
            } else {
                *machine = EngageMachine::Drawing { remaining };
                S::Engaging
            }
        }
        EngageMachine::Engaged => {
            if want_engaged {
                return S::Engaged;
            }
            let len = transition_len("out0");
            if len > 0.0 {
                *machine = EngageMachine::Sheathing { remaining: len };
                S::Disengaging
            } else {
                *machine = EngageMachine::NotEngaged;
                S::NotEngaged
            }
        }
        EngageMachine::Sheathing { remaining } => {
            if want_engaged {
                let len = transition_len("in 0");
                if len > 0.0 {
                    *machine = EngageMachine::Drawing { remaining: len };
                    return S::Engaging;
                }
                *machine = EngageMachine::Engaged;
                return S::Engaged;
            }
            let remaining = remaining - elapsed_frames;
            if remaining <= 0.0 {
                *machine = EngageMachine::NotEngaged;
                S::NotEngaged
            } else {
                *machine = EngageMachine::Sheathing { remaining };
                S::Disengaging
            }
        }
    }
}

fn advance_actor_pose(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    materials: &mut Assets<FfxiSkinnedMaterial>,

    look: Option<(Mat4, Vec3)>,
) {
    let action_id = match actor.action.as_mut() {
        Some(act) => {
            act.remaining -= elapsed_frames;
            if act.remaining <= 0.0 {
                actor.action = None;
                actor.action_clips.clear();
                None
            } else {
                Some(act.clip_id)
            }
        }
        None => None,
    };

    let engage_overlay = match actor.engage {
        EngageMachine::Drawing { .. } => {
            routine_motion_clip(&actor.routines, DatId::from_str("in 0"))
        }
        EngageMachine::Sheathing { .. } => {
            routine_motion_clip(&actor.routines, DatId::from_str("out0"))
        }
        _ => None,
    };

    // research/xim Actor.kt:361 (updateFishingState) — the fishing macro-pose overrides
    // locomotion/idle/rest. fsh0 (cast/wait) and fsh1 (fighting) loop; fsh2..fsh6
    // (resolution) play once and hold (see the one-shot handling below).
    let fishing = actor
        .inputs
        .fishing_phase
        .and_then(actor_state::fishing_clip);

    let (selected_id, is_idle) = if let Some(id) = action_id {
        (id, false)
    } else if let Some(id) = engage_overlay {
        (id, false)
    } else if let Some(fc) = fishing {
        (fc.id, fc.looping)
    } else {
        let rest_id = advance_rest_phase(
            &mut actor.rest_phase,
            actor.inputs.rest,
            &actor.animations,
            elapsed_frames,
        );
        match rest_id {
            Some(rest_id) => (rest_id, true),
            None => {
                let s = actor_state::selected_animation(&actor.inputs);
                (s.id, s.idle)
            }
        }
    };

    let use_battle = actor.action.is_some()
        || !matches!(actor.engage, EngageMachine::NotEngaged)
        || actor.inputs.engage_state.is_battle_idle();
    let overlay: &[SkeletonAnimation] = if use_battle { &actor.battle_clips } else { &[] };
    // Skill-DAT (localDir) clips win over the actor's own pose set, per XIM resolution order.
    let matches: Vec<SkeletonAnimation> = if !actor.action_clips.is_empty() {
        let mut overlaid = actor.action_clips.clone();
        overlaid.extend_from_slice(overlay);
        select_pose_clips_layered(&actor.animations, &overlaid, selected_id)
    } else {
        select_pose_clips_layered(&actor.animations, overlay, selected_id)
    };

    if !matches.is_empty() && actor.current_clip != Some((selected_id, use_battle)) {
        actor.current_clip = Some((selected_id, use_battle));

        let mut new_mask = 0u8;
        for clip in &matches {
            let slot = (clip.id.final_digit().unwrap_or(0) as usize).min(7);
            new_mask |= 1 << slot;
        }

        let old_mask = actor.coordinator.occupied_slots();
        for slot in 0..8usize {
            if old_mask & (1 << slot) != 0 && new_mask & (1 << slot) == 0 {
                actor.coordinator.clear_slot(slot);
            }
        }

        if is_idle {
            for clip in &matches {
                actor
                    .coordinator
                    .register_idle_animation(clip.clone(), true);
            }
        } else {
            // research/xim EffectRoutineInterpolatedEffects.kt:50-51 — when the pose came from
            // a completion motion, honor its parsed transition + loop params; otherwise use the
            // locomotion crossfade defaults.
            let action = actor.action.filter(|a| a.clip_id == selected_id);
            let tp = TransitionParams {
                transition_in_time: action.map_or(LOCOMOTION_XFADE_IN, |a| a.transition_in),
                transition_out_time: action.map_or(LOCOMOTION_XFADE_OUT, |a| a.transition_out),
                ..Default::default()
            };
            // Fishing resolution clips (fsh2..fsh6) have no ActionPlayback, so without an
            // explicit single loop they would default to looping forever — they must play
            // once and hold the final frame until the server advances the state.
            let one_shot_fishing = matches!(fishing, Some(fc) if !fc.looping);
            let loop_params = LoopParams {
                loop_duration: None,
                num_loops: action
                    .and_then(|a| a.num_loops)
                    .or(one_shot_fishing.then_some(1)),
                low_priority: false,
            };
            for clip in &matches {
                actor.coordinator.register_animation(
                    clip.clone(),
                    loop_params,
                    Some(tp.clone()),
                    |_| true,
                );
            }
        }
    }

    actor.last_clip = matches
        .iter()
        .max_by_key(|a| a.key_frame_sets.len())
        .map(|a| a.id);

    actor.coordinator.update(elapsed_frames);

    actor.last_frame = actor
        .coordinator
        .animations
        .iter()
        .flatten()
        .filter_map(|a| a.current_animation.as_ref().map(|c| c.current_frame))
        .next_back()
        .unwrap_or(0.0);

    let mut pose = {
        let coordinator = &actor.coordinator;
        pose_world(
            &actor.skeleton,
            |joint| coordinator.get_joint_transform(joint),
            RootTransform {
                facing_dir: actor.facing_dir,
                skew: 0.0,
                slope_oriented: false,
                scale: Vec3::splat(actor.scale),
            },
            &[],
        )
    };

    if let Some(neck) = actor.head_neck {
        let neck_pose = pose
            .get(neck)
            .map(|m| m.w_axis.truncate())
            .unwrap_or(Vec3::ZERO);
        let desired = match look {
            Some((actor_world, target_world)) => {
                desired_head_rot(actor_world, neck_pose, target_world)
            }
            None => Quat::IDENTITY,
        };
        let alpha = (1.0 - (-elapsed_frames / HEAD_SLEW_TAU_FRAMES).exp()).clamp(0.0, 1.0);
        actor.head_rot = actor.head_rot.slerp(desired, alpha);
        apply_head_look(&mut pose, neck, &actor.head_subtree, actor.head_rot);
    }

    for handle in &actor.materials {
        if let Some(m) = materials.get_mut_untracked(handle) {
            m.joints.set_from(&pose);
        }
    }
}

// Measured from the real skeletons (examples/zz-head-axis, all races): in pose
// space every humanoid stands with up -Y and faces +X (confirmed live: aiming
// the opposite axis inverts the head-look 180°). The head turns relative to the
// body, so the look rotation maps this forward onto the target bearing and is
// applied rigidly to the neck subtree.
const POSE_FORWARD: Vec3 = Vec3::X;
const POSE_UP: Vec3 = Vec3::NEG_Y;
const HEAD_VIEW_CONE_COS: f32 = -0.30;
const HEAD_MAX_TURN_RAD: f32 = 1.20;
const HEAD_SLEW_TAU_FRAMES: f32 = 6.0;

fn desired_head_rot(actor_world: Mat4, neck_pose: Vec3, target_world: Vec3) -> Quat {
    let target_pose = actor_world.inverse().transform_point3(target_world);
    let look = (target_pose - neck_pose).normalize_or_zero();
    if look == Vec3::ZERO || POSE_FORWARD.dot(look) < HEAD_VIEW_CONE_COS {
        return Quat::IDENTITY;
    }
    let rot = roll_free_look(POSE_FORWARD, look, POSE_UP);
    let (axis, angle) = rot.to_axis_angle();
    if angle > HEAD_MAX_TURN_RAD {
        Quat::from_axis_angle(axis, HEAD_MAX_TURN_RAD)
    } else {
        rot
    }
}

/// Maps `from` onto `to` as a yaw about `up` then an in-plane pitch. A
/// minimal-arc rotation would cock the head when the target is both off-center
/// and off-level; the yaw/pitch split keeps the pitch axis horizontal.
fn roll_free_look(from: Vec3, to: Vec3, up: Vec3) -> Quat {
    let f = from.normalize_or_zero();
    let t = to.normalize_or_zero();
    if f == Vec3::ZERO || t == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    let yaw = match (
        (f - up * f.dot(up)).try_normalize(),
        (t - up * t.dot(up)).try_normalize(),
    ) {
        (Some(fh), Some(th)) => Quat::from_rotation_arc(fh, th),
        _ => Quat::IDENTITY,
    };
    let f_yawed = (yaw * f).normalize_or_zero();
    let pitch = if f_yawed == Vec3::ZERO {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(f_yawed, t)
    };
    pitch * yaw
}

#[cfg(test)]
mod head_look_tests {
    use super::*;

    // Pose space (measured): forward +X, up -Y, so a horizontal "right" is
    // up x forward. Targets in front have a positive x.
    fn aim(target: Vec3) -> Quat {
        desired_head_rot(Mat4::IDENTITY, Vec3::ZERO, target)
    }

    #[test]
    fn aims_forward_axis_at_an_in_cone_target() {
        // Target in front (+X), to the side and above: the head's forward axis
        // ends up pointing at it.
        let target = Vec3::new(2.0, 0.6, 1.0);
        let aimed = aim(target) * POSE_FORWARD;
        assert!(
            aimed.abs_diff_eq(target.normalize(), 1e-3),
            "aim wrong: {aimed:?}"
        );
    }

    #[test]
    fn level_target_is_pure_yaw_no_roll() {
        // Level (same height) target: a pure yaw about pose-up, so pose-up is
        // unchanged — the head turns without cocking.
        let up = aim(Vec3::new(2.0, 0.0, 1.5)) * POSE_UP;
        assert!(
            up.abs_diff_eq(POSE_UP, 1e-4),
            "up moved on a level target: {up:?}"
        );
    }

    #[test]
    fn target_behind_view_cone_returns_identity() {
        // Directly behind (-X) is outside the forward cone → no head turn.
        assert_eq!(aim(Vec3::new(-5.0, 0.0, 0.0)), Quat::IDENTITY);
    }

    #[test]
    fn lateral_sweep_never_rolls_the_head() {
        // Sweep the bearing across the front at a fixed elevation; the head's
        // right axis must stay horizontal (no roll) through the whole sweep.
        let right = POSE_UP.cross(POSE_FORWARD).normalize();
        for i in -6..=6 {
            let z = i as f32 * 0.3;
            let r = aim(Vec3::new(2.0, 0.6, z)) * right;
            assert!(
                r.dot(POSE_UP).abs() < 1e-3,
                "rolled at z={z}: {}",
                r.dot(POSE_UP)
            );
        }
    }
}

// Bounds per-frame asset-add + entity-spawn cost when several loads finish at
// once (zone-in floods); the rest stay queued and drain on subsequent frames.
const ACTOR_SPAWNS_PER_FRAME: usize = 2;

#[derive(Resource, Default)]
pub struct ActorLoadInFlight {
    tasks: HashMap<u32, Task<Result<PreparedActor, String>>>,
    keys: HashMap<u32, ActorPrepKey>,
    ready: std::collections::VecDeque<(u32, Arc<PreparedActor>)>,
    cache: ActorPrepCache,
}

pub fn kick_load_actor_tasks(
    mut events: MessageReader<LoadActorRequest>,
    tracked: Res<crate::scene::TrackedEntities>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut in_flight: ResMut<ActorLoadInFlight>,
) {
    let quality = crate::zone_texture::TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    for req in events.read() {
        if !tracked.by_id.contains_key(&req.entity_id) {
            continue;
        }
        let key = prep_key(&req.subject, quality);
        if let Some(prepared) = in_flight.cache.get_and_promote(&key) {
            in_flight.tasks.remove(&req.entity_id);
            in_flight.keys.remove(&req.entity_id);
            in_flight.ready.retain(|(id, _)| *id != req.entity_id);
            in_flight.ready.push_back((req.entity_id, prepared));
            continue;
        }
        let subject = req.subject.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let loaded = match subject {
                ActorSubject::Npc { file_id } => load_npc(file_id),
                ActorSubject::Pc {
                    race,
                    equipment,
                    main_weapon,
                    sub_weapon,
                } => load_pc(race, &equipment, main_weapon, sub_weapon),
            }?;
            let parts = prepare_actor_parts(&loaded, 0.0, 1.0, quality);
            Ok(PreparedActor { loaded, parts })
        });
        // Newest look wins: replacing the entry drops any stale in-flight load.
        in_flight.tasks.insert(req.entity_id, task);
        in_flight.keys.insert(req.entity_id, key);
        in_flight.ready.retain(|(id, _)| *id != req.entity_id);
    }
}

pub fn poll_load_actor_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    tracked: Res<crate::scene::TrackedEntities>,
    entity_mesh: Option<Res<crate::scene::EntityMesh>>,
    mut in_flight: ResMut<ActorLoadInFlight>,
    q_existing: Query<&FfxiRenderRoot>,
    q_ball: Query<&MeshMaterial3d<StandardMaterial>, With<Mesh3d>>,
) {
    if in_flight.tasks.is_empty() && in_flight.ready.is_empty() {
        return;
    }
    // EntityMesh only exists once a scene is loaded; park finished tasks until then.
    let Some(entity_mesh) = entity_mesh else {
        return;
    };
    let mut completed: Vec<(u32, Result<PreparedActor, String>)> = Vec::new();
    in_flight.tasks.retain(
        |entity_id, task| match future::block_on(future::poll_once(task)) {
            Some(res) => {
                completed.push((*entity_id, res));
                false
            }
            None => true,
        },
    );
    for (entity_id, prepared) in completed {
        let key = in_flight.keys.remove(&entity_id);
        match prepared {
            Ok(p) => {
                let p = Arc::new(p);
                if let Some(key) = key {
                    in_flight.cache.insert(key, Arc::clone(&p));
                }
                in_flight.ready.retain(|(id, _)| *id != entity_id);
                in_flight.ready.push_back((entity_id, p));
            }
            Err(e) => {
                warn!("ffxi actor load failed (entity {entity_id}): {e}");
            }
        }
    }
    for _ in 0..ACTOR_SPAWNS_PER_FRAME {
        let Some((entity_id, prepared)) = in_flight.ready.pop_front() else {
            break;
        };
        // The wire entity may have despawned (or been re-tracked) while the load
        // ran; resolve it fresh and drop the result if it is gone.
        let Some(&wire_entity) = tracked.by_id.get(&entity_id) else {
            continue;
        };

        if let Ok(FfxiRenderRoot(old_root)) = q_existing.get(wire_entity) {
            commands.entity(*old_root).try_despawn();
        }

        let root = spawn_live_actor(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &prepared,
            wire_entity,
            entity_id,
            1.0,
        );

        // A transient child carries the stretch: the wire entity is driven by
        // sync and the model shares its transform, so neither can be reshaped.
        // A reload has no resting orb to consume and just regrows the model.
        let orb = q_ball.get(wire_entity).ok().and_then(|mm| {
            let lit = std_materials.get(&mm.0).map(|m| {
                let mut m = m.clone();
                m.alpha_mode = AlphaMode::Blend;
                m
            })?;
            let emissive = lit.emissive;
            let handle = std_materials.add(lit);
            commands.entity(wire_entity).remove::<Mesh3d>();
            let orb = commands
                .spawn((
                    Mesh3d(entity_mesh.morph_orb.clone()),
                    MeshMaterial3d(handle.clone()),
                    Transform::from_xyz(0.0, MORPH_COLUMN_PIVOT_Y, 0.0),
                    Visibility::Visible,
                    bevy::light::NotShadowCaster,
                    ChildOf(wire_entity),
                ))
                .id();
            Some((orb, handle, emissive))
        });

        commands.entity(root).insert(Transform {
            translation: Vec3::ZERO,
            rotation: ffxi_to_bevy_basis(),
            scale: Vec3::splat(MORPH_START_SCALE),
        });

        commands.entity(wire_entity).try_insert((
            FfxiRenderRoot(root),
            crate::components::MorphIn {
                elapsed: 0.0,
                actor_root: root,
                orb: orb.as_ref().map(|(e, _, _)| *e),
                orb_mat: orb.as_ref().map(|(_, h, _)| h.clone()),
                orb_emissive: orb.map(|(_, _, e)| e).unwrap_or(LinearRgba::BLACK),
            },
        ));
    }
}

const MORPH_START_SCALE: f32 = 0.03;
const MORPH_DURATION: f32 = 0.5;
const MORPH_COLUMN_PIVOT_Y: f32 = 1.0;
const MORPH_COLUMN_STRETCH: f32 = 11.0;

fn ease_out_back(p: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.0;
    let x = p - 1.0;
    1.0 + C3 * x * x * x + C1 * x * x
}

pub fn tick_morph_in(
    time: Res<Time>,
    mut commands: Commands,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut q_morph: Query<(Entity, &mut crate::components::MorphIn)>,
    mut q_tf: Query<&mut Transform>,
) {
    let dt = time.delta_secs();
    for (wire_entity, mut morph) in &mut q_morph {
        morph.elapsed += dt;
        let p = (morph.elapsed / MORPH_DURATION).clamp(0.0, 1.0);

        // The figure rises into the column over the back three-quarters.
        let emerge = ((p - 0.25) / 0.75).clamp(0.0, 1.0);
        let grow = MORPH_START_SCALE + (1.0 - MORPH_START_SCALE) * ease_out_back(emerge);
        if let Ok(mut tf) = q_tf.get_mut(morph.actor_root) {
            tf.scale = Vec3::splat(grow);
        }

        // Ball -> vertical light-column -> nothing: stretch up, then thin away.
        let stretch = (p / 0.5).clamp(0.0, 1.0);
        let collapse = ((p - 0.4) / 0.6).clamp(0.0, 1.0);
        let sy = 1.0 + (MORPH_COLUMN_STRETCH - 1.0) * stretch;
        let sxz = 1.0 - collapse;
        if let Some(orb) = morph.orb {
            if let Ok(mut tf) = q_tf.get_mut(orb) {
                tf.scale = Vec3::new(sxz, sy, sxz);
            }
        }
        if let Some(handle) = &morph.orb_mat {
            if let Some(mat) = std_materials.get_mut(handle) {
                let fade = sxz;
                let e = morph.orb_emissive;
                mat.base_color = mat.base_color.with_alpha(fade);
                mat.emissive = LinearRgba::new(e.red * fade, e.green * fade, e.blue * fade, 1.0);
            }
        }

        if p >= 1.0 {
            if let Ok(mut tf) = q_tf.get_mut(morph.actor_root) {
                tf.scale = Vec3::ONE;
            }
            if let Some(orb) = morph.orb {
                commands.entity(orb).try_despawn();
            }
            commands
                .entity(wire_entity)
                .remove::<crate::components::MorphIn>();
        }
    }
}

// Map an observed entity's broadcast animation byte (server_status / ANIMATIONTYPE)
// to its persistent rest pose. `/heal` and `/sit` ride the same animation channel
// the server uses for engage and fishing; SITCHAIR is left unmapped (needs a
// chair-anchored clip). vendor/server/src/map/entities/baseentity.h.
fn observed_rest_kind(animation: u8) -> ffxi_actor::actor_state::RestKind {
    use ffxi_actor::actor_state::RestKind;
    use ffxi_proto::decode::animation;
    match animation {
        animation::HEALING => RestKind::Heal,
        animation::SIT => RestKind::Sit,
        _ => RestKind::None,
    }
}

pub fn tick_live_ffxi_actors(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    motion: Res<combat_stance::EntityMotion>,
    rest: Res<combat_stance::RestStance>,
    walk_mode: Res<combat_stance::WalkMode>,
    self_move: Res<combat_stance::SelfMoveIntent>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    target: Res<crate::scene::Target>,
    mut q_actors: Query<(&mut FfxiRenderActor, &GlobalTransform)>,

    mut prev_zone: Local<Option<Option<u16>>>,
) {
    use ffxi_actor::actor_state::RestKind;

    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    let self_id = state.snapshot.self_char_id;

    let pos_by_id: std::collections::HashMap<u32, ffxi_viewer_wire::Vec3> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.pos))
        .collect();

    // Head-look must aim at where the target is *rendered* (grounded), not its
    // raw wire Y — the server sends pathing NPCs a flat reference Y, so wire and
    // rendered Y diverge after snap_entities_to_mzb_floor_system.
    let actor_world_by_id: std::collections::HashMap<u32, Vec3> = q_actors
        .iter()
        .map(|(a, gt)| (a.world_id, gt.translation()))
        .collect();

    // Engaged combat stance is the server's animation byte (ANIMATION_ATTACK),
    // set on every entity at engage and broadcast in the General block — see LSB
    // CBattleEntity::OnEngage, vendor/server/src/map/entities/baseentity.h. The
    // reactor goal only *predicts* self-engage for snappy feedback before the
    // server echoes, and only some UIs set it, so it can't be the source of truth.
    let self_engaged_predicted = matches!(
        state.snapshot.current_goal,
        Some(ffxi_viewer_wire::ReactorGoal::Engaged { .. })
    );
    let self_reactor_driven = !matches!(
        state.snapshot.current_goal,
        None | Some(ffxi_viewer_wire::ReactorGoal::Idle)
    );

    let zone = state.snapshot.zone_id;
    let zone_changed = matches!(*prev_zone, Some(p) if p != zone);
    *prev_zone = Some(zone);

    let present: std::collections::HashSet<u32> =
        state.snapshot.entities.iter().map(|e| e.id).collect();

    // Head-look: facetarget is a targid (act_index), so resolve it to the world_id
    // the position maps are keyed by. Distinct from bt_target_id (the combat-claim
    // UniqueNo), which only turns the head mid-combat and lives in a different
    // id-space — see vendor/server char_update.cpp Flags0.facetarget.
    let face_target_by_id: std::collections::HashMap<u32, u16> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.face_target))
        .collect();
    let id_by_targid: std::collections::HashMap<u16, u32> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.act_index, e.id))
        .collect();

    let engaged_by_id: std::collections::HashMap<u32, bool> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.animation == ffxi_proto::decode::animation::ATTACK))
        .collect();

    let dead_by_id: std::collections::HashMap<u32, bool> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.hp_pct == Some(0)))
        .collect();

    // Fishing macro-pose for observed players: the server broadcasts the fsh* state in
    // the entity's animation byte (server_status). Self drives its pose from the local
    // mini-game instead, so it is excluded below.
    let fishing_phase_by_id: std::collections::HashMap<u32, Option<u8>> = state
        .snapshot
        .entities
        .iter()
        .map(|e| {
            (
                e.id,
                ffxi_proto::decode::animation::fishing_phase(e.animation),
            )
        })
        .collect();

    // Resting pose for observed players: the server broadcasts /heal and /sit in the
    // entity's animation byte (server_status), the same channel as engage and fishing.
    // Self drives its own rest pose from local input (RestStance), so the wire byte is
    // consulted only for others.
    let rest_kind_by_id: std::collections::HashMap<u32, RestKind> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, observed_rest_kind(e.animation)))
        .collect();

    // Self KO is unreliable via the entity hp_pct (only updated when CHAR_PC
    // carries UPDATE_HP) and via the party row (absent/stale when solo).
    // death_homepoint_secs comes straight from 0x037 CHAR_STATUS hpp==0.
    let self_dead = state.snapshot.death_homepoint_secs.is_some()
        || crate::hud::self_hud::resolve_self(&state.snapshot.party, self_id)
            .map(|m| m.hp_pct == 0)
            .unwrap_or(false);

    for (mut actor, actor_global) in &mut q_actors {
        let world_id = actor.world_id;
        if world_id == 0 {
            continue;
        }

        let is_self = Some(world_id) == self_id;

        if zone_changed || (!is_self && !present.contains(&world_id)) {
            actor.inputs = ActorAnimInputs::default();
            actor.rest_phase = RestPlayback::Inactive;

            actor.action = None;
            actor.engage = EngageMachine::NotEngaged;
            actor.coordinator.clear();
            actor.current_clip = None;
            advance_actor_pose(&mut actor, elapsed_frames, &mut materials, None);
            continue;
        }

        let sample = motion.sample(world_id).unwrap_or_default();

        let engaged = engaged_by_id.get(&world_id).copied().unwrap_or(false)
            || (is_self && self_engaged_predicted);
        let dead = (is_self && self_dead) || dead_by_id.get(&world_id).copied().unwrap_or(false);

        let rest_kind = if is_self {
            match rest.kind {
                combat_stance::RestKind::None => RestKind::None,
                combat_stance::RestKind::Sit => RestKind::Sit,
                combat_stance::RestKind::Heal => RestKind::Heal,
            }
        } else {
            rest_kind_by_id
                .get(&world_id)
                .copied()
                .unwrap_or(RestKind::None)
        };

        let (forward_vel, strafe_vel) = if engaged && !is_self {
            (sample.forward_component, sample.strafe_component)
        } else {
            (0.0, 0.0)
        };

        let walking = if is_self {
            walk_mode.walking
        } else {
            infers_walk_gait(sample.speed)
        };

        // Self pose comes from the local mini-game machine (it knows the active reeling
        // sub-states the server never broadcasts); others come from the wire animation byte.
        let fishing_phase = if is_self {
            state.snapshot.self_fishing.map(|f| f.phase)
        } else {
            fishing_phase_by_id.get(&world_id).copied().flatten()
        };

        let engage_state = {
            let actor: &mut FfxiRenderActor = &mut actor;
            advance_engage(
                &mut actor.engage,
                engaged,
                &actor.routines,
                &actor.battle_clips,
                elapsed_frames,
            )
        };

        actor.facing_dir = 0.0;
        actor.inputs = ActorAnimInputs {
            moving: if is_self && !self_reactor_driven {
                self_move.moving
            } else {
                motion.is_moving(world_id)
            },
            walking,
            forward_vel,
            strafe_vel,
            heading_rate: sample.heading_rate,
            engage_state,
            dead,
            rest: rest_kind,
            fishing_phase,
            ..Default::default()
        };

        let look_target_id = if is_self {
            target.id
        } else {
            face_target_by_id
                .get(&world_id)
                .copied()
                .filter(|&t| t != 0)
                .and_then(|targid| id_by_targid.get(&targid).copied())
        };
        let look = look_target_id
            .filter(|&tid| tid != world_id)
            .and_then(|tid| {
                actor_world_by_id
                    .get(&tid)
                    .copied()
                    .or_else(|| pos_by_id.get(&tid).map(|&w| crate::scene::ffxi_to_bevy(w)))
            })
            .map(|base| {
                let world = base + Vec3::Y * TARGET_LOOK_HEIGHT;
                (actor_global.to_matrix(), world)
            });

        advance_actor_pose(&mut actor, elapsed_frames, &mut materials, look);
    }
}

const TARGET_LOOK_HEIGHT: f32 = 1.4;

const CAST_TIMEOUT_FRAMES: f32 = 60.0 * FRAME_RATE;

pub fn dispatch_action_overlay(
    events: Res<crate::snapshot::EventLog>,
    mut q_actors: Query<&mut FfxiRenderActor>,
    mut last_seen: Local<u64>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let ffxi_viewer_wire::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
        } = *ev
        else {
            continue;
        };
        let Some(mut actor) = q_actors.iter_mut().find(|a| a.world_id == actor_id) else {
            continue;
        };

        match action_routine(action_kind, action_id) {
            None => {
                if actor.action.map(|a| a.looping).unwrap_or(false) {
                    actor.action = None;
                }
            }
            Some((routine, looping)) => {
                let Some(clip_id) = routine_motion_clip(&actor.routines, routine) else {
                    continue;
                };

                let len = rest_clip_len_frames(&actor.battle_clips, clip_id)
                    .max(rest_clip_len_frames(&actor.animations, clip_id));
                let remaining = if looping {
                    CAST_TIMEOUT_FRAMES
                } else {
                    len.max(1.0)
                };
                actor.action = Some(ActionPlayback {
                    clip_id,
                    looping,
                    remaining,
                    num_loops: None,
                    transition_in: LOCOMOTION_XFADE_IN,
                    transition_out: LOCOMOTION_XFADE_OUT,
                });
            }
        }
    }
}

pub fn update_ffxi_render_actor_lighting(
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    ambient: Res<GlobalAmbientLight>,
    zone_lighting: Res<crate::weather::ZoneDirectionalLighting>,
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
    q_actors: Query<&FfxiRenderActor>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    const AMBIENT_REF_LUX: f32 = 1000.0;
    const DIR_REF_LUX: f32 = 12000.0;

    const COLOR_BIAS: Vec3 = Vec3::new(1.4, 1.36, 1.45);
    const AMBIENT_BIAS_BELOW: f32 = 0.5;
    const AMBIENT_FLOOR: f32 = 0.12;
    // The 0x2F entity sun/moon diffuse is authored overbright (up to ~1.27 at noon);
    // clamping the model directional to 1.0 cropped that punch and flattened the form.
    const MODEL_DIR_MAX: f32 = 1.5;

    // research/xim EnvironmentSection.kt:144-148: actors are lit by the model block's
    // entity ambient. When the zone ships 0x2F records, use that authored ambient
    // directly — the data already carries the day/night level and a ~2.4:1 sun:ambient
    // ratio, so scaling it by GlobalAmbientLight (amb_k) and the dark-fallback
    // COLOR_BIAS only lifted the shadow side and flattened the model's form.
    let mut amb_rgb = if zone_lighting.valid {
        zone_lighting.ambient_entity
    } else {
        let amb = ambient.color.to_linear();
        let amb_k = (ambient.brightness / AMBIENT_REF_LUX).clamp(0.0, 1.5);
        let mut a = Vec3::new(amb.red, amb.green, amb.blue) * amb_k;
        if a.max_element() < AMBIENT_BIAS_BELOW {
            a *= COLOR_BIAS;
        }
        a
    };
    amb_rgb = amb_rgb.max(Vec3::splat(AMBIENT_FLOOR));
    let ambient_v = amb_rgb.extend(1.0);

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
    // research/xim EnvironmentSection.kt:161-165: actors take a single time-blended
    // model light (the moon<->sun cross-fade), so dir0 carries the blend and dir1 is
    // unused. The procedural sun/moon DirectionalLights remain the fallback when the
    // zone ships no 0x2F records.
    let (dir0_dir, dir0_color, dir1_dir, dir1_color) = if zone_lighting.valid {
        let (md, mc) = if zone_lighting.model_dir != Vec3::ZERO && zone_lighting.model_k > 0.0 {
            let f = (-zone_lighting.model_dir).normalize_or_zero();
            let c = zone_lighting.model_color;
            (
                Vec4::new(f.x, f.y, f.z, 0.0),
                Vec4::new(
                    c.x,
                    c.y,
                    c.z,
                    zone_lighting.model_k.clamp(0.0, MODEL_DIR_MAX),
                ),
            )
        } else {
            (Vec4::ZERO, Vec4::ZERO)
        };
        (md, mc, Vec4::ZERO, Vec4::ZERO)
    } else {
        let (d0d, d0c) = extract(q_sun.single().ok());
        let (d1d, d1c) = extract(q_moon.single().ok());
        (d0d, d0c, d1d, d1c)
    };

    let realistic = if settings.realistic_character_lighting {
        1.0
    } else {
        0.0
    };

    let receive = if settings.faithful_shadow_receive {
        1.0
    } else {
        0.0
    };

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
            if let Some(m) = materials.get_mut_untracked(h) {
                m.lighting = lighting.clone();

                m.material_flags.flags.y = realistic;
                m.material_flags.flags.z = receive;
            }
        }
    }
}

pub fn update_ffxi_actor_point_lights(
    active: Res<crate::zone_point_lights::ActiveSceneLights>,
    q_actors: Query<(&FfxiRenderActor, &GlobalTransform)>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    if active.lights.is_empty() {
        return;
    }

    for (actor, gt) in &q_actors {
        let (point_pos, point_color, point_atten) =
            crate::zone_point_lights::nearest_point_light_arrays(gt.translation(), &active.lights);

        for h in &actor.materials {
            if let Some(m) = materials.get_mut_untracked(h) {
                m.lighting.point_pos = point_pos;
                m.lighting.point_color = point_color;
                m.lighting.point_atten = point_atten;
            }
        }
    }
}

pub fn add_tick_system(app: &mut App) {
    app.add_systems(Update, tick_ffxi_render_actors);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoseState {
    Idle,
    Walk,
    Run,
    StrafeLeft,
    StrafeRight,
    Back,
    Sit,
    Kneel,
    Heal,
    Dead,
}

impl PoseState {
    pub fn label(self) -> &'static str {
        match self {
            PoseState::Idle => "idle",
            PoseState::Walk => "walk",
            PoseState::Run => "run",
            PoseState::StrafeLeft => "strafeL",
            PoseState::StrafeRight => "strafeR",
            PoseState::Back => "back",
            PoseState::Sit => "sit",
            PoseState::Kneel => "kneel",
            PoseState::Heal => "heal",
            PoseState::Dead => "dead",
        }
    }

    pub fn from_name(s: &str) -> Option<PoseState> {
        Some(match s {
            "idle" => PoseState::Idle,
            "walk" => PoseState::Walk,
            "run" => PoseState::Run,
            "strafeL" | "strafel" => PoseState::StrafeLeft,
            "strafeR" | "strafer" => PoseState::StrafeRight,
            "back" => PoseState::Back,
            "sit" => PoseState::Sit,
            "kneel" => PoseState::Kneel,
            "heal" => PoseState::Heal,
            "dead" => PoseState::Dead,
            _ => return None,
        })
    }
}

pub fn inputs_for_pose(state: PoseState, engaged: bool) -> ActorAnimInputs {
    use ffxi_actor::actor_state::{EngageAnimationState, RestKind};

    let mut inputs = ActorAnimInputs {
        engage_state: if engaged {
            EngageAnimationState::Engaged
        } else {
            EngageAnimationState::NotEngaged
        },
        ..Default::default()
    };

    match state {
        PoseState::Idle => {}
        PoseState::Walk => {
            inputs.moving = true;
            inputs.walking = true;
        }
        PoseState::Run => {
            inputs.moving = true;
            inputs.forward_vel = 1.0;
        }
        PoseState::StrafeLeft => {
            inputs.moving = true;

            inputs.forward_vel = -0.5;
            inputs.strafe_vel = -1.0;
        }
        PoseState::StrafeRight => {
            inputs.moving = true;
            inputs.forward_vel = 0.0;
            inputs.strafe_vel = 1.0;
        }
        PoseState::Back => {
            inputs.moving = true;
            inputs.forward_vel = -1.0;
        }
        PoseState::Sit => inputs.rest = RestKind::Sit,
        PoseState::Kneel => inputs.rest = RestKind::Kneel,
        PoseState::Heal => inputs.rest = RestKind::Heal,
        PoseState::Dead => inputs.dead = true,
    }

    inputs
}

#[cfg(test)]
mod pose_resolution_tests {

    use super::*;
    use ffxi_actor::actor_state::ActorAnimInputs;

    fn resolved_clip_ids(actor: &LoadedActor, inputs: &ActorAnimInputs) -> Vec<String> {
        let animations = actor.all_animations();
        let battle = actor.all_battle_clips();

        let overlay: &[SkeletonAnimation] = if inputs.engage_state.is_battle_idle() {
            &battle
        } else {
            &[]
        };
        let selected_id = match actor_state::rest_animation_id(inputs.rest) {
            Some(rest_id) => rest_id,
            None => actor_state::selected_animation(inputs).id,
        };
        let mut ids: Vec<String> = select_pose_clips_layered(&animations, overlay, selected_id)
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn load_hume_m() -> Option<LoadedActor> {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return None;
        }

        Some(load_pc(1, &[], None, None).expect("load Hume M"))
    }

    #[test]
    fn run_composites_both_layers() {
        let Some(actor) = load_hume_m() else { return };
        let ids = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Run, false));
        assert!(
            ids.contains(&"run0".to_string()) && ids.contains(&"run1".to_string()),
            "casual run must register run0+run1 (got {ids:?})"
        );
    }

    #[test]
    fn casual_set_excludes_battle_clips_and_run_differs() {
        let Some(actor) = load_hume_m() else { return };
        let casual: Vec<String> = actor
            .all_animations()
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        let battle: Vec<String> = actor
            .all_battle_clips()
            .iter()
            .map(|a| a.id.as_str())
            .collect();

        assert!(
            casual.contains(&"run1".to_string()),
            "casual set has casual run1 (got {casual:?})"
        );
        assert!(
            !casual.iter().any(|s| s.starts_with("btl")),
            "casual set must exclude battle idle (got {casual:?})"
        );
        assert!(
            !casual.iter().any(|s| s.starts_with("at0")),
            "casual set must exclude swings (got {casual:?})"
        );

        assert!(
            battle.iter().any(|s| s.starts_with("btl")),
            "battle overlay has btl"
        );
        assert!(
            battle.contains(&"run1".to_string()),
            "battle overlay has drawn-stance run1"
        );

        let run1 =
            |set: &[SkeletonAnimation]| set.iter().find(|a| a.id.as_str() == "run1").cloned();
        let c = run1(&actor.all_animations()).unwrap();
        let b = run1(&actor.all_battle_clips()).unwrap();
        assert!(
            c.num_frames != b.num_frames
                || c.key_frame_duration != b.key_frame_duration
                || c.key_frame_sets.len() != b.key_frame_sets.len(),
            "casual run1 must be a distinct clip from the battle run1"
        );
    }

    #[test]
    fn engaged_idle_differs_from_casual_idle() {
        let Some(actor) = load_hume_m() else { return };
        let idle = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Idle, false));
        let battle = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Idle, true));
        assert_ne!(idle, battle, "engaged idle must switch idl?->btl?");
        assert!(
            idle.iter().any(|s| s.starts_with("idl")),
            "casual idle = idl? (got {idle:?})"
        );
        assert!(
            battle.iter().any(|s| s.starts_with("btl")),
            "engaged idle = btl? (got {battle:?})"
        );
    }

    #[test]
    fn walk_differs_from_run() {
        let Some(actor) = load_hume_m() else { return };
        let run = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Run, false));
        let walk = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Walk, false));
        assert_ne!(run, walk, "walk must be a different clip set than run");
        assert!(
            walk.contains(&"wlk0".to_string()) && walk.contains(&"wlk1".to_string()),
            "walk must register wlk0+wlk1 (got {walk:?})"
        );
    }

    #[test]
    fn rest_poses_resolve_to_layered_clips() {
        let Some(actor) = load_hume_m() else { return };

        let sit = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Sit, false));
        assert!(
            sit.contains(&"si00".to_string()) && sit.contains(&"si01".to_string()),
            "/sit must register si00+si01 (got {sit:?})"
        );

        let kneel = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Kneel, false));
        let heal = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Heal, false));
        assert!(
            kneel.contains(&"rx00".to_string()) && kneel.contains(&"rx01".to_string()),
            "/kneel must register rx00+rx01 (got {kneel:?})"
        );
        assert_eq!(kneel, heal, "/heal and /kneel share the rx0? kneel pose");

        let idle = resolved_clip_ids(&actor, &inputs_for_pose(PoseState::Idle, false));
        assert_ne!(sit, idle, "/sit must not fall back to idle");
        assert_ne!(kneel, idle, "/kneel must not fall back to idle");
    }

    #[test]
    fn observed_rest_kind_maps_broadcast_animation_byte() {
        use ffxi_proto::decode::animation;
        assert_eq!(observed_rest_kind(animation::HEALING), RestKind::Heal);
        assert_eq!(observed_rest_kind(animation::SIT), RestKind::Sit);
        assert_eq!(observed_rest_kind(animation::NONE), RestKind::None);
        assert_eq!(observed_rest_kind(animation::ATTACK), RestKind::None);
    }

    #[test]
    fn rest_phase_machine_sequences_in_loop_out() {
        let anims: Vec<SkeletonAnimation> = Vec::new();
        let mut phase = RestPlayback::Inactive;
        let step = |phase: &mut RestPlayback, desired| {
            advance_rest_phase(phase, desired, &anims, 1.0).map(|d| d.as_str())
        };

        assert_eq!(step(&mut phase, RestKind::Kneel).as_deref(), Some("rx0?"));
        assert_eq!(step(&mut phase, RestKind::Kneel).as_deref(), Some("rx1?"));
        assert_eq!(step(&mut phase, RestKind::Kneel).as_deref(), Some("rx1?"));

        assert_eq!(step(&mut phase, RestKind::None).as_deref(), Some("rx2?"));
        assert_eq!(step(&mut phase, RestKind::None), None);
        assert_eq!(step(&mut phase, RestKind::None), None);
    }

    fn synth_routines(pairs: &[(&[u8; 4], &[u8; 4])]) -> HashMap<DatId, Scheduler> {
        use ffxi_dat::scheduler::{SchedulerStage, TimedStage};
        let mut out = HashMap::new();
        for &(name, clip) in pairs {
            out.insert(
                DatId::from_name(name),
                Scheduler {
                    name: *name,
                    stages: vec![TimedStage {
                        frame: 0,
                        stage: SchedulerStage {
                            kind: StageKind::Motion,
                            raw_type: 0x05,
                            delay_frames: 0,
                            duration_frames: 0,
                            id: *clip,
                            max_loops: 0,
                            transition_in: 0,
                            transition_out: 0,
                        },
                    }],
                },
            );
        }
        out
    }

    fn synth_anim(id: &[u8; 4], length: usize) -> SkeletonAnimation {
        SkeletonAnimation {
            id: DatId::from_name(id),
            num_joints: 0,
            num_frames: length + 1,
            key_frame_duration: 1.0,
            key_frame_sets: Default::default(),
        }
    }

    #[test]
    fn routine_motion_clip_resolves_first_motion_stage() {
        let routines = synth_routines(&[(b"ati0", b"at0?"), (b"in 0", b"ind?")]);
        assert_eq!(
            routine_motion_clip(&routines, DatId::from_str("ati0")).map(|d| d.as_str()),
            Some("at0?".to_string())
        );

        assert_eq!(
            routine_motion_clip(&routines, DatId::from_str("in 0")).map(|d| d.as_str()),
            Some("ind?".to_string())
        );

        assert_eq!(
            routine_motion_clip(&routines, DatId::from_str("cawh")),
            None
        );
    }

    #[test]
    fn action_routing_maps_categories() {
        let r = |k, id| action_routine(k, id).map(|(d, looping)| (d.as_str(), looping));

        assert_eq!(r(1, 0), Some(("ati0".to_string(), false)));

        assert_eq!(r(8, 1), Some(("cawh".to_string(), true)));

        assert_eq!(r(8, 144), Some(("cabk".to_string(), true)));

        assert_eq!(r(8, 0xFFFF), Some(("cast".to_string(), true)));

        assert_eq!(r(10, 0), Some(("cast".to_string(), true)));
        assert_eq!(r(12, 0), Some(("calg".to_string(), true)));
        assert_eq!(r(9, 0), Some(("cait".to_string(), false)));

        for finish in [2u8, 3, 4, 5, 6, 0] {
            assert_eq!(r(finish, 0), None, "category {finish} should not pose");
        }
    }

    #[test]
    fn engage_machine_draws_then_sheathes() {
        use actor_state::EngageAnimationState as S;
        let routines = synth_routines(&[(b"in 0", b"ind?"), (b"out0", b"otd?")]);

        let anims = vec![synth_anim(b"ind0", 2), synth_anim(b"otd0", 1)];
        let mut m = EngageMachine::NotEngaged;
        let step = |m: &mut EngageMachine, want| advance_engage(m, want, &routines, &anims, 1.0);

        assert_eq!(step(&mut m, true), S::Engaging);
        assert_eq!(step(&mut m, true), S::Engaging);
        assert_eq!(step(&mut m, true), S::Engaged);
        assert_eq!(step(&mut m, true), S::Engaged);

        assert_eq!(step(&mut m, false), S::Disengaging);
        assert_eq!(step(&mut m, false), S::NotEngaged);
        assert_eq!(step(&mut m, false), S::NotEngaged);
    }

    #[test]
    fn engage_machine_snaps_when_transition_clip_absent() {
        use actor_state::EngageAnimationState as S;

        let routines = synth_routines(&[]);
        let anims: Vec<SkeletonAnimation> = Vec::new();
        let mut m = EngageMachine::NotEngaged;
        assert_eq!(
            advance_engage(&mut m, true, &routines, &anims, 1.0),
            S::Engaged
        );
        assert_eq!(
            advance_engage(&mut m, false, &routines, &anims, 1.0),
            S::NotEngaged
        );
    }

    #[test]
    fn real_routines_resolve_to_clips() {
        let Some(actor) = load_hume_m() else { return };
        let routines = actor.all_routines();
        let clip = |routine: &str| {
            routine_motion_clip(&routines, DatId::from_str(routine)).map(|d| d.as_str())
        };

        assert_eq!(clip("ati0").as_deref(), Some("at0?"), "swing routine");
        assert_eq!(clip("in 0").as_deref(), Some("ind?"), "draw routine");
        assert_eq!(clip("out0").as_deref(), Some("otd?"), "sheathe routine");
        assert_eq!(
            clip("cawh").as_deref(),
            Some("mw0?"),
            "white-magic cast routine"
        );

        let swing = routine_motion_clip(&routines, DatId::from_str("ati0")).unwrap();
        let anims = actor.all_animations();
        let battle = actor.all_battle_clips();
        let ids: Vec<String> = select_pose_clips_layered(&anims, &battle, swing)
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert!(
            ids.contains(&"at00".to_string()) && ids.contains(&"at01".to_string()),
            "swing resolves to at00+at01 (got {ids:?})"
        );
    }
}
