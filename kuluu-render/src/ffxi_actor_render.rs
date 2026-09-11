#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};

use ffxi_actor::actor_state::{self, ActorAnimInputs, RestKind};
use ffxi_actor::animation::{LoopParams, SkeletonAnimationCoordinator, TransitionParams};
use ffxi_actor::skeleton_instance::{
    apply_head_look, find_head_neck, neck_subtree, pose_world, pose_world_mounted_into,
    standard_joint_world_position, MountAttach, PoseScratch, RootTransform,
};

use ffxi_dat::cib::{Cib, MovementType};
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
pub use crate::combat_stance::{infers_walk_gait, WALK_RUN_BOUNDARY};
use crate::dat_vos2::skeleton_file_id_for_race;
use crate::scene::BakedActor;
use crate::skinned_ffxi_material::{
    FfxiInstance, FfxiInstanceSlot, FfxiJointMatrices, FfxiLightingUniform, FfxiSkinRegistry,
    FfxiSkinSlot, FfxiSkinnedMaterial, FfxiSkinnedMaterialCache, ATTR_COLOR, ATTR_JOINT0,
    ATTR_JOINT1, ATTR_JOINT_WEIGHT, ATTR_NORMAL0, ATTR_NORMAL1, ATTR_POSITION0, ATTR_POSITION1,
};

#[derive(Debug, Clone)]
pub enum ActorSubject {
    Pc {
        race: u8,
        /// Loads the race's mount-pose animation DAT alongside the usual motion
        /// ones, which is where a rider's `chi?` seat and the other mount poses
        /// live (research/xim poc/Model.kt, PcModel.getMountAnimationResource).
        mounted: bool,
        equipment: Vec<u32>,
        /// Body slot, kept apart from `equipment` because its CIB `waist_type`
        /// picks the waist motion DAT (SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel collects it
        /// from slot 2 specifically).
        body: Option<u32>,
        main_weapon: Option<u32>,
        sub_weapon: Option<u32>,
    },

    Npc {
        file_id: u32,
    },

    /// A ridden mount whose model is a PC race config rather than an NPC model.
    /// Only the chocobo is built this way in retail — one race per coat colour,
    /// with the body parts coming from the equipment table like a PC's gear.
    /// research/xim poc/Model.kt, RaceGenderConfig.
    Mount {
        race: u8,
    },
}

#[derive(Message, Debug, Clone)]
pub struct LoadActorRequest {
    pub entity_id: u32,
    pub subject: ActorSubject,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct FfxiRenderRoot(pub Entity);

// The skeleton domain ticks at half the routine clock (research/xim poc/ActorManager.kt updateAll);
// every `half_frames()`/`* 0.5` conversion in this module is that same 2:1 bridge.
pub const FRAME_RATE: f32 =
    crate::scheduler_runtime::ROUTINE_FPS / crate::scheduler_runtime::SKELETON_FRAME_DIVISOR;

pub const LOCOMOTION_XFADE_IN: f32 = 9.0;

pub const LOCOMOTION_XFADE_OUT: f32 = 7.5;

fn special_log_enabled() -> bool {
    tracing::enabled!(target: "special", tracing::Level::DEBUG)
}

// Tick counter for the gated hold probe below; advanced once per snapshot tick in
// `tick_live_ffxi_actors` (serial section), read from the parallel pose pass.
static SPECIAL_LOG_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Once-per-(world_id, clip, reason) dedupe for CLIP_WARN: a miss repeats every frame while the
// pose is held, and the diagnosis only needs the first sighting per entity per requested clip.
// The reason stays in the key so two distinct diagnostics on one pair (a not_found that also
// leaves current_clip untouched) both get their line instead of deduping into one.
static CLIP_WARN_SEEN: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<(u32, DatId, &'static str)>>,
> = std::sync::OnceLock::new();

// Returns true when this call printed the line (first sighting of the pair), false when the
// dedupe set already had it. Callers do not branch on it; tests use it to pin the dedupe key.
fn clip_warn_once(id: u32, model: &str, clip: &DatId, reason: &'static str) -> bool {
    let seen = CLIP_WARN_SEEN.get_or_init(Default::default);
    let Ok(mut guard) = seen.lock() else {
        return false;
    };
    if !guard.insert((id, *clip, reason)) {
        return false;
    }
    tracing::warn!(
        target: "clip",
        id = format_args!("{id:#x}"),
        model,
        clip = %clip.as_str(),
        reason,
        "CLIP_WARN"
    );
    true
}

// CLIP_WARN reason for a tier whose requested clip resolved to no usable chunk: seq_load_error
// when the model's DAT walk saw the chunk but parse rejected it, not_found_override_skipped when
// an override tier (special/fishing) asked for a clip the model does not ship, and not_found
// otherwise. The reason stays in the dedupe key so two distinct misses on one pair both print.
fn clip_miss(id: u32, model: &str, clip: &DatId, rejected_clips: &[DatId], tier: PoseTier) {
    let reason = if rejected_clips.iter().any(|r| r.parameterized_match(clip)) {
        "seq_load_error"
    } else if matches!(tier, PoseTier::Special | PoseTier::Fishing) {
        "not_found_override_skipped"
    } else {
        "not_found"
    };
    clip_warn_once(id, model, clip, reason);
}

fn clip_ok(id: u32, asked: &DatId, resolved: &SkeletonAnimation, movement_type: MovementType) {
    tracing::debug!(
        target: "clip",
        id = format_args!("{id:#x}"),
        clip = %asked.as_str(),
        resolved = %resolved.id.as_str(),
        frames = resolved.num_frames,
        movement = %movement_type,
        "CLIP_OK"
    );
}

pub(crate) fn ffxi_to_bevy_basis() -> Quat {
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

    pub animations: Arc<Vec<SkeletonAnimation>>,

    battle_clips: Arc<Vec<SkeletonAnimation>>,

    routines: Arc<HashMap<DatId, Scheduler>>,

    // Particle generators + their sprite meshes/textures embedded in the actor
    // DAT; auto-run generators (research/xim Actor.kt startAutoRunParticles) start at spawn.
    action_assets: Arc<crate::scheduler_runtime::ActionAssets>,

    /// Motion chunks present in this model's DAT walk whose parse yielded no usable frames
    /// (zero key frame sets or zero frames). Kept out of `animations`'s usability, not out of
    /// the vec itself: CLIP_WARN must tell "the chunk is there but broken" (seq_load_error)
    /// apart from "the model ships no such clip at all" (not_found).
    rejected_clips: Vec<DatId>,

    /// The primary model DAT as `{rom_dir}/{dir}/{file}.DAT` (e.g. ROM/4/109.DAT), for the
    /// CLIP_WARN line's `model=` field.
    model_dat: String,

    /// The model's 0x45 Info chunk when its primary DAT carries one (vekien/xi-model-viewer
    /// ui/js/dat/inspect.js parseInspectInfo). Mounts are None on purpose: their Info layout is
    /// the `mount` variant, whose +0x0A byte is a pose type, not a scale (research/xim
    /// resource/InfoSection.kt readMountDefinition).
    cib: Option<Cib>,
}

// A parsed motion chunk the pose pass can actually play: at least one key frame set and at
// least two frames. `skel_anim::parse` never fails; it returns an empty SkeletonAnimation for
// bad data, so a zero-frame or joint-less entry in `animations` is a parse rejection, not a
// clip the model ships.
fn is_usable_clip(anim: &SkeletonAnimation) -> bool {
    !anim.key_frame_sets.is_empty() && anim.num_frames > 0
}

// Clip/scheduler parsing is the expensive tail of an actor load; deriving it here
// keeps it on the loader task instead of the render main thread, and the Arcs let
// consumers share the parsed sets without deep-cloning keyframe data. The fourth
// return value lists the motion chunks seen in the DAT walk but rejected by parse,
// so CLIP_WARN can name a seq_load_error instead of a not_found.
fn derive_animation_sets(
    anim_dirs: &[ResourceDir],
    battle_dirs: &[ResourceDir],
) -> (
    Arc<Vec<SkeletonAnimation>>,
    Arc<Vec<SkeletonAnimation>>,
    Arc<HashMap<DatId, Scheduler>>,
    Vec<DatId>,
) {
    let animations = dedup_clips(anim_dirs.iter());
    let battle_clips = dedup_clips(battle_dirs.iter());
    let rejected_clips: Vec<DatId> = animations
        .iter()
        .chain(battle_clips.iter())
        .filter(|a| !is_usable_clip(a))
        .map(|a| a.id)
        .collect();
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
        rejected_clips,
    )
}

// The CLIP_WARN `model=` label for a file id: `{rom_dir}/{dir}/{file}.DAT`, the same join as
// `DatLocation::join_under` without the install root.
fn model_dat_label(root: &DatRoot, file_id: u32) -> String {
    match root.resolve(file_id) {
        Ok(loc) => format!(
            "{}/{}/{}.DAT",
            loc.rom_dir, loc.sub_path.dir, loc.sub_path.file
        ),
        Err(_) => "?".to_string(),
    }
}

// research/xim EffectRoutineInstance.kt searchAssociatedDir — a sound id in a routine
// resolves against every one of the actor's resource dirs. For a PC that is where the whole
// melee sound set lives: `skaz`/`shit` in the equipped weapon's DAT, `atk1..atk4`/`dam1..dam4`
// in the FACE model DAT. Only Sep and Generator chunks are collected; the Img/D3M/MMB decode
// that `parse_action_bytes` also does is the actor loader's expensive tail and is not needed to
// turn a stage id into an se_id (ffxi_dat::action::resolve_stage_to_se).
fn collect_sound_assets(dirs: &[&[ResourceDir]]) -> crate::scheduler_runtime::ActionAssets {
    let mut assets = crate::scheduler_runtime::ActionAssets::default();
    for dir in dirs.iter().flat_map(|d| d.iter()) {
        for c in ffxi_dat::chunk::walk(dir.bytes()).flatten() {
            match ffxi_dat::kind::ChunkKind::from_u8(c.kind) {
                Some(ffxi_dat::kind::ChunkKind::Sep) => {
                    if let Ok(sep) = ffxi_dat::sep::Sep::parse(c.name, c.data) {
                        assets.seps.entry(c.name).or_insert(sep);
                    }
                }
                Some(ffxi_dat::kind::ChunkKind::Generator) => {
                    if let Ok(Some(g)) = ffxi_dat::generator::Generator::parse(c.name, c.data) {
                        assets.generators.entry(c.name).or_insert(g);
                    }
                }
                _ => {}
            }
        }
    }
    assets
}

// Everything CPU-heavy about turning a LoadedActor into spawnable pieces —
// vertex conversion, mip-chain generation, bind pose — happens here so the
// loader task pays it, not the render main thread.
pub struct PreparedParts {
    images: Vec<Image>,

    skel_built: Vec<BuiltGroup>,

    d3m_built: Vec<BuiltGroup>,

    bind_joints: FfxiJointMatrices,

    /// Bind-pose bounds of the assembled actor in bevy space (feet at y≈0),
    /// computed with the same facing/scale as `bind_joints` — i.e. the mesh as
    /// drawn. Feeds camera and hitbox bounds.
    pub bounds: Option<(Vec3, Vec3)>,
}

pub struct PreparedActor {
    pub loaded: LoadedActor,
    parts: PreparedParts,

    /// The model's transform scale, resolved once at load time. 1.0 for PCs and mounts;
    /// the 0x45 Info `scale` byte divided by 100 for NPC models (see kick_load_actor_tasks).
    pub scale: f32,
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
                tint: crate::skinned_ffxi_material::t_factor_tint(
                    buffer.render_properties.t_factor,
                ),
                joint_aabbs: skel_joint_bounds(buffer, joint_count),
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
            tint: Vec4::ONE,
            joint_aabbs: d3m_joint_bounds(d3m),
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
        bounds: loaded.bind_pose_bounds(facing_dir, scale),
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
    Mount {
        race: u8,
        mipmaps: bool,
        anisotropy: u16,
    },
    Pc {
        race: u8,
        mounted: bool,
        equipment: Vec<u32>,
        body: Option<u32>,
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
        ActorSubject::Mount { race } => ActorPrepKey::Mount {
            race: *race,
            mipmaps: q.mipmaps,
            anisotropy: q.anisotropy,
        },
        ActorSubject::Pc {
            race,
            mounted,
            equipment,
            body,
            main_weapon,
            sub_weapon,
        } => ActorPrepKey::Pc {
            race: *race,
            mounted: *mounted,
            equipment: equipment.clone(),
            body: *body,
            main_weapon: *main_weapon,
            sub_weapon: *sub_weapon,
            mipmaps: q.mipmaps,
            anisotropy: q.anisotropy,
        },
    }
}

const ACTOR_PREP_CACHE_CAP: usize = 48;

struct ActorPrepEntry {
    prepared: Arc<PreparedActor>,
    // Filled on first spawn: every later spawn of this look reuses the same
    // Mesh assets, so Bevy's batcher can group their draws (same pipeline +
    // material + mesh) instead of encoding one draw per fresh Mesh handle.
    mesh_handles: Vec<Handle<Mesh>>,
}

#[derive(Default)]
struct ActorPrepCache {
    map: HashMap<ActorPrepKey, ActorPrepEntry>,
    order: std::collections::VecDeque<ActorPrepKey>,
}

impl ActorPrepCache {
    fn get_and_promote(&mut self, key: &ActorPrepKey) -> Option<Arc<PreparedActor>> {
        let hit = Arc::clone(&self.map.get(key)?.prepared);
        self.order.retain(|k| k != key);
        self.order.push_back(key.clone());
        Some(hit)
    }

    fn insert(&mut self, key: ActorPrepKey, prepared: Arc<PreparedActor>) {
        let entry = ActorPrepEntry {
            prepared,
            mesh_handles: Vec::new(),
        };
        if self.map.insert(key.clone(), entry).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > ACTOR_PREP_CACHE_CAP {
            let Some(evict) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&evict);
        }
    }

    fn mesh_handles(
        &mut self,
        key: &ActorPrepKey,
        meshes: &mut Assets<Mesh>,
    ) -> Option<Vec<Handle<Mesh>>> {
        let entry = self.map.get_mut(key)?;
        if entry.mesh_handles.is_empty() {
            entry.mesh_handles = add_part_meshes(&entry.prepared.parts, meshes);
        }
        Some(entry.mesh_handles.clone())
    }
}

fn add_part_meshes(parts: &PreparedParts, meshes: &mut Assets<Mesh>) -> Vec<Handle<Mesh>> {
    parts
        .skel_built
        .iter()
        .chain(parts.d3m_built.iter())
        .map(|b| meshes.add(b.mesh.clone()))
        .collect()
}

fn read_dat(root: &DatRoot, file_id: u32) -> Option<Vec<u8>> {
    let loc = root.resolve(file_id).ok()?;
    fs::read(loc.path_under(root)).ok()
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

    let (_schedulers, action_assets) = crate::scheduler_runtime::parse_action_bytes(&bytes);
    // A D3m referenced by a particle generator is drawn by the particle stream
    // (XIM ParticleMeshResource, Particle.kt shouldSnapAlpha) with its own unlit additive/blend
    // material; rendering it as a static child too would double-draw it through the
    // lit skinned-Mask path (blowing sparse halos to white slabs and back-faces to
    // black — kuluu-xvym). The Home Point crystal is entirely such meshes: even the
    // gem shard reads solid only because it is a large closed mesh drawn additively,
    // not because it depth-writes (every generator has depthMask=0).
    let particle_meshes: std::collections::HashSet<[u8; 4]> = action_assets
        .particle_defs
        .values()
        .map(|d| d.mesh_id)
        .collect();
    effect_meshes.retain(|d| !particle_meshes.contains(&d.name));

    let anim_dirs = vec![ResourceDir::from_bytes(bytes)];
    let (animations, battle_clips, routines, rejected_clips) =
        derive_animation_sets(&anim_dirs, &[]);
    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,
        effect_meshes,
        textures,
        animations,
        battle_clips,
        routines,
        action_assets: Arc::new(action_assets),
        rejected_clips,
        model_dat: model_dat_label(&root, file_id),
        cib: dir.first_cib(),
    })
}

/// Ridden-chocobo race configs, one per coat colour, paired with the equipment
/// table row its body parts come from. Retail's race index and equipment row
/// diverge for every non-playable config, so the pairing is data, not arithmetic
/// (research/xim poc/Model.kt, RaceGenderConfig).
const CHOCOBO_RACE_TABLE: [(u8, u8); 5] = [(32, 12), (33, 13), (34, 14), (35, 15), (36, 16)];

/// The body slots a chocobo is assembled from. It has no face row and carries no
/// weapons, so the playable races' 0 and 6..=8 are simply absent from its block
/// of the equipment lookup table.
const MOUNT_BODY_SLOTS: std::ops::RangeInclusive<u8> = 1..=5;

pub fn chocobo_race_for_colour(colour: kuluu_snapshot::ChocoboColour) -> u8 {
    use kuluu_snapshot::ChocoboColour as C;
    let index = match colour {
        C::Yellow => 0,
        C::Black => 1,
        C::Blue => 2,
        C::Red => 3,
        C::Green => 4,
    };
    CHOCOBO_RACE_TABLE[index].0
}

fn mount_equipment_table_index(race: u8) -> Option<u8> {
    CHOCOBO_RACE_TABLE
        .iter()
        .find_map(|&(r, table)| (r == race).then_some(table))
}

/// A mount built from a PC race config: the skeleton and its `chi?`/run/walk
/// clips come from the race DAT, the body parts from the race's equipment table
/// row at model id 0 — a rented chocobo wears none of the trait variants.
pub fn load_mount_race(race: u8) -> Result<LoadedActor, String> {
    crate::perf_probe::note_model_load();
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let dll =
        ffxi_dat::main_dll::MainDll::load(root.root()).map_err(|e| format!("FFXiMain.dll: {e}"))?;
    let table_index = mount_equipment_table_index(race)
        .ok_or_else(|| format!("race {race} is not a mount race config"))?;
    let skel_file_id = u32::from(
        dll.base_race_config_index(race)
            .ok_or_else(|| format!("no race-config table entry for mount race {race}"))?,
    );

    let skel_bytes = read_dat(&root, skel_file_id)
        .ok_or_else(|| format!("read mount race dat {skel_file_id}"))?;
    let skeleton = first_skeleton(&skel_bytes)
        .ok_or_else(|| format!("no skeleton in mount race dat {skel_file_id}"))?;

    let mut textures = Vec::new();
    let mut skel_meshes = Vec::new();
    let mut anim_dirs = vec![ResourceDir::from_bytes(skel_bytes.clone())];
    collect_textures(&walk_tree(&skel_bytes), &mut textures);

    let mut unrendered: Vec<u32> = Vec::new();
    for slot in MOUNT_BODY_SLOTS {
        let Some(file_id) = dll.equipment_model_index(table_index, slot, 0) else {
            continue;
        };
        let Some(bytes) = read_dat(&root, file_id) else {
            unrendered.push(file_id);
            continue;
        };
        let meshes = ResourceDir::from_bytes(bytes.clone()).collect_skel_meshes();
        if meshes.is_empty() {
            unrendered.push(file_id);
            continue;
        }
        skel_meshes.extend(meshes);
        collect_textures(&walk_tree(&bytes), &mut textures);
        anim_dirs.push(ResourceDir::from_bytes(bytes));
    }
    if !unrendered.is_empty() {
        warn!("load_mount_race race={race}: body files resolved but unrendered {unrendered:?}");
    }
    if skel_meshes.is_empty() {
        return Err(format!("no body meshes for mount race {race}"));
    }

    let (animations, battle_clips, routines, rejected_clips) =
        derive_animation_sets(&anim_dirs, &[]);
    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,
        effect_meshes: Vec::new(),
        textures,
        animations,
        battle_clips,
        routines,
        action_assets: Arc::new(collect_sound_assets(&[&anim_dirs])),
        rejected_clips,
        model_dat: model_dat_label(&root, skel_file_id),
        // The mount's own Info chunk uses the `mount` layout (rotation/poseType at +0x02/+0x0A,
        // research/xim resource/InfoSection.kt readMountDefinition); parsing it with the info
        // layout would misread poseType as a scale byte, so mounts carry no CIB.
        cib: None,
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

// research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp SkeletalMeshActor::GetUpperBodyDatIndex
// and :3165 — the two companion motion DATs sit at fixed offsets from the race
// skeleton base, indexed by a CIB byte.
const CIB_MOTION_INDEX_NONE: u8 = 0xFF;
const UPPER_BODY_MOTION_OFFSET: u32 = 1;
const WAIST_MOTION_OFFSET: u32 = 2;
// `GetWaistDatIndex` floors waist_type at 1 before using it (:3134-3136), so an
// unequipped or CIB-less body still resolves to the trousers waist rather than
// colliding with the upper-body DAT.
const WAIST_TYPE_MIN: u8 = 1;

/// One DAT off the race's action-animation base.
/// research/xim poc/Model.kt, PcModel.getMountAnimationResource.
fn action_anim_dat(root: &DatRoot, race: u8, offset: u16) -> Option<Vec<u8>> {
    let dll = ffxi_dat::main_dll::MainDll::load(root.root())
        .inspect_err(|e| warn!("FFXiMain.dll unreadable, action poses unavailable: {e}"))
        .ok()?;
    let base = dll.base_action_animation_index(race)?;
    read_dat(root, u32::from(base + offset))
}

pub fn load_pc(
    race: u8,
    mounted: bool,
    equipment: &[u32],
    body: Option<u32>,
    main_weapon: Option<u32>,

    sub_weapon: Option<u32>,
) -> Result<LoadedActor, String> {
    crate::perf_probe::note_model_load();
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let skel_file_id =
        skeleton_file_id_for_race(race).ok_or_else(|| format!("unsupported race {race}"))?;

    let skel_bytes =
        read_dat(&root, skel_file_id).ok_or_else(|| format!("read skel dat {skel_file_id}"))?;
    let skeleton = first_skeleton(&skel_bytes)
        .ok_or_else(|| format!("no skeleton in race dat {skel_file_id}"))?;

    let mut skel_meshes = Vec::new();
    let mut textures = Vec::new();
    let mut anim_dirs = vec![ResourceDir::from_bytes(skel_bytes.clone())];

    // The race skeleton DAT's Info chunk is the PC's movement info: retail copies its
    // movementType but drops the scale byte (research/xim poc/Model.kt PcModel.getMovementInfo),
    // so a PC never renders at the race CIB's 82-95 percent.
    let race_cib;
    {
        let dir = ResourceDir::from_bytes(skel_bytes.clone());
        skel_meshes.extend(dir.collect_skel_meshes());
        collect_textures(&walk_tree(&skel_bytes), &mut textures);
        race_cib = dir.first_cib();
    }

    // Retail loads three motion DATs around the race base, not one. Upper body is
    // `base + is_shield + 1` (SkeletalMeshActor.cpp SkeletalMeshActor::GetUpperBodyDatIndex) — a shield swaps in a
    // variant with its own joint count. Waist/skirt is
    // `base + max(waist_type, 1) + 2` (:3165, reached from ReadStdMotionRes at
    // :3014), which drives the hip-hung cloth joints; without it they hold bind
    // pose through every idle, walk, run, strafe and death.
    //
    // Both selectors come from equipment CIBs and neither is a fixed offset: the
    // sub slot supplies is_shield and the body slot waist_type
    // (SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel,1682 collect them per slot). Loading a fixed
    // `+3` instead would give every robed mage trouser motion, and because
    // `dedup_clips` is first-writer-wins, loading both candidates would silently
    // keep whichever came first rather than the authored one.
    let cib_byte = |file_id: Option<u32>, pick: fn(&ffxi_dat::cib::Cib) -> u8| {
        file_id
            .and_then(|f| read_dat(&root, f))
            .map(ResourceDir::from_bytes)
            .and_then(|d| d.first_cib())
            .map(|c| pick(&c))
            .unwrap_or(0)
    };
    let is_shield = cib_byte(sub_weapon, |c| c.is_shield);
    let waist_type = cib_byte(body, |c| c.body_armour_waist).max(WAIST_TYPE_MIN);

    for offset in [
        u32::from(is_shield) + UPPER_BODY_MOTION_OFFSET,
        u32::from(waist_type) + WAIST_MOTION_OFFSET,
    ] {
        if let Some(bytes) = read_dat(&root, skel_file_id + offset) {
            anim_dirs.push(ResourceDir::from_bytes(bytes));
        }
    }

    // The seat poses (`chi?` for a chocobo, `{n}un?` for the other mounts) plus
    // their own run/walk variants ship in one DAT off the race's action-animation
    // base, and only while riding does retail put it in the animation set.
    if mounted {
        match action_anim_dat(&root, race, ffxi_dat::main_dll::ACTION_ANIM_MOUNT_OFFSET) {
            Some(bytes) => anim_dirs.insert(0, ResourceDir::from_bytes(bytes)),
            None => warn!("load_pc race={race}: no mount-pose DAT — rider will not sit"),
        }
    }

    // Fishing is not a load-time property the way mounting is — any PC in view
    // can start a cast at any time, and an actor is built once — so the fishing
    // DAT rides along for every PC. It is a quarter the size of the race base
    // and carries the `fsh*` routines the pose selector resolves through.
    match action_anim_dat(&root, race, ffxi_dat::main_dll::ACTION_ANIM_FISHING_OFFSET) {
        Some(bytes) => anim_dirs.push(ResourceDir::from_bytes(bytes)),
        None => warn!("load_pc race={race}: no fishing DAT — fishing poses unavailable"),
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
        if weapon_anim_type != 0 && weapon_anim_type != CIB_MOTION_INDEX_NONE {
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

    let (animations, battle_clips, routines, rejected_clips) =
        derive_animation_sets(&anim_dirs, &battle_dirs);
    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,

        effect_meshes: Vec::new(),
        textures,
        animations,
        battle_clips,
        routines,
        action_assets: Arc::new(collect_sound_assets(&[&anim_dirs, &battle_dirs])),
        rejected_clips,
        model_dat: model_dat_label(&root, skel_file_id),
        cib: race_cib,
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
    // Per-mesh t_factor tint, neutral 1.0 (research/xim GLDrawer.kt drawXimSkinned meshColor; D3M
    // children carry no RenderProperties record). displayTypeFlag is slot
    // occlusion (ActorModel.kt isOccluded renderProperties, `is_occluded`), not a blend selector —
    // skinned meshes always alpha-test at SKINNED_ALPHA_DISCARD
    // (SkeletonMeshSection.kt parseTriStrip); translucency/glow comes from the particle
    // stream, never the static mesh.
    tint: Vec4,

    joint_aabbs: Arc<[JointLocalAabb]>,
}

fn clamp_joint(idx: u16, joint_count: usize) -> u32 {
    let i = idx as usize;
    if i < joint_count {
        i as u32
    } else {
        0
    }
}

// Influences below JOINT_WEIGHT_EPS are skipped (the weight-pre-scaled p0/p1
// cannot be divided by ~0); a skipped term shifts the skinned position by at
// most EPS * the actor's extent, which ACTOR_AABB_MARGIN absorbs.
const JOINT_WEIGHT_EPS: f32 = 1e-4;
const ACTOR_AABB_MARGIN: f32 = 0.05;

#[derive(Clone, Copy, Debug)]
pub(crate) struct JointLocalAabb {
    joint: u32,
    min: Vec3,
    max: Vec3,
}

#[derive(Default)]
struct JointBoundsAccum(HashMap<u32, (Vec3, Vec3)>);

impl JointBoundsAccum {
    fn add(&mut self, joint: u32, p: Vec3) {
        let e = self
            .0
            .entry(joint)
            .or_insert((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)));
        e.0 = e.0.min(p);
        e.1 = e.1.max(p);
    }

    fn finish(self) -> Arc<[JointLocalAabb]> {
        let mut boxes: Vec<JointLocalAabb> = self
            .0
            .into_iter()
            .map(|(joint, (min, max))| JointLocalAabb { joint, min, max })
            .collect();
        boxes.sort_by_key(|b| b.joint);
        boxes.into()
    }
}

// Re-expresses bevy_mesh-0.19.0/src/skinning.rs:120-171 (SkinnedMeshBounds::
// from_mesh) for the FFXI dual-influence stream: p0/p1 are joint-local
// positions pre-scaled by their weight (skinned_ffxi.wgsl header), so the
// weight divides back out to recover the unweighted point each joint box must
// bound. The skinned position is a convex combination of the two
// joint-transformed points, so the union of transformed boxes bounds every pose.
fn skel_joint_bounds(buffer: &MeshBuffer, joint_count: usize) -> Arc<[JointLocalAabb]> {
    let mut accum = JointBoundsAccum::default();
    for v in &buffer.vertices {
        let w0 = v.joint0_weight;
        let w1 = 1.0 - w0;
        if w0 > JOINT_WEIGHT_EPS {
            accum.add(
                clamp_joint(v.joint_index0, joint_count),
                Vec3::from(v.p0) / w0,
            );
        }
        if w1 > JOINT_WEIGHT_EPS {
            accum.add(
                clamp_joint(v.joint_index1, joint_count),
                Vec3::from(v.p1) / w1,
            );
        }
    }
    accum.finish()
}

fn d3m_joint_bounds(d3m: &D3m) -> Arc<[JointLocalAabb]> {
    let mut accum = JointBoundsAccum::default();
    for v in &d3m.vertices {
        accum.add(0, Vec3::from(v.pos));
    }
    accum.finish()
}

fn entity_aabb_from_joints(
    joints: &FfxiJointMatrices,
    joint_aabbs: &[JointLocalAabb],
) -> Option<Aabb> {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    let mut any = false;
    for b in joint_aabbs {
        let Some(m) = joints.matrices.get(b.joint as usize) else {
            continue;
        };
        let center = m.transform_point3((b.min + b.max) * 0.5);
        let half = (b.max - b.min) * 0.5;
        let extent = m.x_axis.truncate().abs() * half.x
            + m.y_axis.truncate().abs() * half.y
            + m.z_axis.truncate().abs() * half.z;
        lo = lo.min(center - extent);
        hi = hi.max(center + extent);
        any = true;
    }
    let margin = Vec3::splat(ACTOR_AABB_MARGIN);
    any.then(|| Aabb::from_min_max(lo - margin, hi + margin))
}

// research/XIClient Rendering/Direct3D8Manager.cpp Direct3D8Manager::InitializeRenderStateBlocks — the skeletal vertex colour reaches
// fixed-function T&L as D3DMCS_COLOR1 exactly as the zone MMB one does, so it takes the same
// D3DCOLOR byte/255 scale (pinned against `mmb::VERTEX_COLOR_DIVISOR` below:
// `ffxi_zone_material::AMBIENT_FLOOR` is chosen for both paths at once and only holds while
// terrain and actors decode alike). Unlike MMB there is no MODULATE4X alpha op on this path, so
// alpha takes the plain divisor rather than MMB's half-scale one.
const ACTOR_VERTEX_COLOR_DIVISOR: f32 = u8::MAX as f32;
const ACTOR_VERTEX_ALPHA_DIVISOR: f32 = ACTOR_VERTEX_COLOR_DIVISOR;

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
            v.color[0] as f32 / ACTOR_VERTEX_COLOR_DIVISOR,
            v.color[1] as f32 / ACTOR_VERTEX_COLOR_DIVISOR,
            v.color[2] as f32 / ACTOR_VERTEX_COLOR_DIVISOR,
            v.color[3] as f32 / ACTOR_VERTEX_ALPHA_DIVISOR,
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
    action_assets: Arc<crate::scheduler_runtime::ActionAssets>,
    rejected_clips: Vec<DatId>,
    model_dat: String,
    coordinator: SkeletonAnimationCoordinator,
    skin_slot: u32,
    instance_slots: Vec<u32>,

    pub inputs: ActorAnimInputs,

    pub world_id: u32,

    pub facing_dir: f32,

    pub scale: f32,

    /// The model's 0x45 Info movement byte (Unset when the DAT carries no CIB). Gates whether
    /// the wire AnimationSpeed stride scale applies to locomotion clip playback.
    movement_type: MovementType,

    current_clip: Option<(DatId, bool)>,

    rest_phase: RestPlayback,

    death_phase: actor_state::DeathPhase,

    engage: EngageMachine,

    action: Option<ActionPlayback>,
    action_clips: Vec<SkeletonAnimation>,

    head_neck: Option<usize>,
    head_subtree: Vec<usize>,

    head_rot: Quat,

    pub last_clip: Option<DatId>,
    pub last_frame: f32,

    world_pose: Vec<Mat4>,
    pose_work: PoseScratch,

    point_light_selection: Option<ActorPointLightSelection>,
}

impl FfxiRenderActor {
    pub fn skin_slot(&self) -> u32 {
        self.skin_slot
    }

    pub fn world_pose(&self) -> &[Mat4] {
        &self.world_pose
    }

    /// The clip id the pose pass currently has selected (None before its first run).
    pub fn current_clip_id(&self) -> Option<&DatId> {
        self.current_clip.as_ref().map(|(id, _)| id)
    }

    /// The 0x45 Info movement byte this model was loaded with (Unset when the DAT carries no
    /// CIB); gates the wire stride scale on locomotion clip playback.
    pub fn movement_type(&self) -> MovementType {
        self.movement_type
    }

    /// The completion motion's clip while `action` is held - what a routine's Motion stage (or
    /// the flinch consumer) just started. None when no completion motion owns the pose.
    pub fn active_action_clip(&self) -> Option<&DatId> {
        self.action.as_ref().map(|a| &a.clip_id)
    }

    pub fn instance_slots(&self) -> &[u32] {
        &self.instance_slots
    }

    pub(crate) fn routines(&self) -> &HashMap<DatId, Scheduler> {
        &self.routines
    }

    // The SEP/generator tier a sound stage resolves against when the running routine's own DAT
    // does not hold it: a PC's grunt SEPs ship in the FACE model DAT and the weapon's swing
    // whoosh in the equipped weapon's DAT (research/xim EffectRoutineInstance.kt searchAssociatedDir
    // searchAssociatedDir over `actor.getAllAnimationDirectories()`).
    pub(crate) fn action_assets(&self) -> &crate::scheduler_runtime::ActionAssets {
        &self.action_assets
    }

    pub(crate) fn cast_posing(&self) -> bool {
        self.action.is_some_and(|a| a.cast_pose)
    }

    /// XIM's `currentlyIdle` (EffectRoutineInterpolatedEffects.kt): every coordinator slot is
    /// null or running a low-priority clip - the idle clips only. The flinch overwrites those
    /// and nothing else, so this is its gate.
    pub fn is_pose_idle(&self) -> bool {
        self.coordinator.animations.iter().flatten().all(|a| {
            a.current_animation
                .as_ref()
                .is_some_and(|c| c.loop_params.low_priority)
        })
    }

    /// The flinch clip this model plays: dfm? for PCs, dfi? otherwise (XIM's `model is PcModel`
    /// test), falling back to the other family when only one ships - HumeM carries no dfi?, and
    /// a mob hit by a PC must still flinch. None when the model carries neither.
    pub fn flinch_clip(&self, pc: bool) -> Option<DatId> {
        let ships = |prefix: &str| {
            self.animations
                .iter()
                .chain(self.battle_clips.iter())
                .any(|a| a.id.starts_with(prefix))
        };
        let order = if pc { ["dfm", "dfi"] } else { ["dfi", "dfm"] };
        order
            .into_iter()
            .find(|prefix| ships(prefix))
            .map(|prefix| DatId::from_str(&format!("{prefix}?")))
    }

    pub fn begin_completion_motion(&mut self, clip_id: DatId, motion: CompletionMotion) {
        // research/xim EffectRoutineInterpolatedEffects.kt SkeletonAnimationInstance animationDirs — a skill's body motion is
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
        // research/xim EffectRoutineInterpolatedEffects.kt SkeletonAnimationInstance - maxLoops
        // passes through to the coordinator verbatim: 0 loops until the effect ends, N ≥ 1 plays
        // N times and pins the end frame (SkeletonAnimator.kt applyLoopBounds).
        let num_loops = (motion.max_loops != 0).then_some(motion.max_loops as u32);
        self.action = Some(ActionPlayback {
            clip_id,
            looping: num_loops.is_some(),
            remaining: len.max(motion.duration_frames * 0.5).max(1.0),
            num_loops,
            transition_in: half_frames(motion.transition_in),
            transition_out: half_frames(motion.transition_out),
            cast_pose: false,
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

    cast_pose: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum RestPlayback {
    Inactive,

    Starting { kind: RestKind, remaining: f32 },

    Looping { kind: RestKind },

    Stopping { kind: RestKind, remaining: f32 },
}

impl LoadedActor {
    /// The model's 0x45 Info chunk when its primary DAT carries one (None for mounts and
    /// any DAT without an Info section).
    pub fn cib(&self) -> Option<&Cib> {
        self.cib.as_ref()
    }

    fn all_animations(&self) -> Arc<Vec<SkeletonAnimation>> {
        Arc::clone(&self.animations)
    }

    fn all_battle_clips(&self) -> Arc<Vec<SkeletonAnimation>> {
        Arc::clone(&self.battle_clips)
    }

    /// The actor's own-DAT routine table. Public for the offline harnesses (the rabbit
    /// tester verifies a model actually ships `bti0` before asserting on its limb clip).
    pub fn all_routines(&self) -> Arc<HashMap<DatId, Scheduler>> {
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

#[allow(clippy::too_many_arguments)]
pub fn spawn_loaded_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    material_cache: &mut FfxiSkinnedMaterialCache,
    registry: &mut FfxiSkinRegistry,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    world_pos: Vec3,
    facing_dir: f32,
    scale: f32,
    q: crate::zone_texture::TextureQuality,
) -> Entity {
    let parts = prepare_actor_parts(loaded, facing_dir, scale, q);
    let mesh_handles = add_part_meshes(&parts, meshes);
    let skin_slot = registry.alloc_skin();
    registry.skin_mut(skin_slot).joints = parts.bind_joints.clone();

    let actor_root = commands
        .spawn((
            Transform {
                translation: world_pos,
                rotation: ffxi_to_bevy_basis(),
                scale: Vec3::ONE,
            },
            GlobalTransform::default(),
            Visibility::default(),
            FfxiSkinSlot(skin_slot),
        ))
        .id();

    let instance_slots = build_actor_children(
        commands,
        &mesh_handles,
        materials,
        material_cache,
        registry,
        images,
        loaded,
        &parts,
        actor_root,
        skin_slot,
    );

    commands.entity(actor_root).insert(make_render_actor(
        loaded,
        skin_slot,
        instance_slots,
        0,
        facing_dir,
        scale,
    ));
    insert_auto_run_effects(commands, actor_root, loaded);

    actor_root
}

// research/xim Actor.kt createFrom — auto-run generators start at model-ready.
fn insert_auto_run_effects(commands: &mut Commands, actor_root: Entity, loaded: &LoadedActor) {
    if loaded
        .action_assets
        .particle_defs
        .values()
        .any(|d| d.auto_run)
    {
        commands
            .entity(actor_root)
            .insert(crate::particle_sim::ActorAutoRunEffects {
                assets: Arc::clone(&loaded.action_assets),
            });
    }
}

#[derive(Component)]
pub struct FfxiActorMeshChild;

#[derive(Component)]
pub(crate) struct ActorMeshJointBounds {
    skin_slot: u32,
    joint_aabbs: Arc<[JointLocalAabb]>,
}

#[allow(clippy::too_many_arguments)]
fn build_actor_children(
    commands: &mut Commands,
    mesh_handles: &[Handle<Mesh>],
    materials: &mut Assets<FfxiSkinnedMaterial>,
    material_cache: &mut FfxiSkinnedMaterialCache,
    registry: &mut FfxiSkinRegistry,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    parts: &PreparedParts,
    actor_root: Entity,
    skin_slot: u32,
) -> Vec<u32> {
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

    let mut instance_slots = Vec::new();

    for (built, mesh_handle) in parts
        .skel_built
        .iter()
        .chain(parts.d3m_built.iter())
        .zip(mesh_handles)
    {
        let untextured = is_blank_texture(&built.texture_name);
        let tex_handle = if untextured {
            None
        } else {
            resolve_texture(&built.texture_name)
        };
        let has_texture = if tex_handle.is_some() { 1.0 } else { 0.0 };

        let mat = material_cache.get_or_create(tex_handle, materials);
        let instance_slot = registry.alloc_instance(FfxiInstance {
            flags: Vec4::new(has_texture, 0.0, 0.0, 0.0),
            tint: built.tint,
            skin_slot,
        });
        instance_slots.push(instance_slot);

        let child = commands
            .spawn((
                Mesh3d(mesh_handle.clone()),
                MeshMaterial3d(mat),
                MeshTag(instance_slot),
                FfxiInstanceSlot(instance_slot),
                Transform::default(),
                FfxiActorMeshChild,
                ChildOf(actor_root),
            ))
            .id();
        if let Some(aabb) = entity_aabb_from_joints(&parts.bind_joints, &built.joint_aabbs) {
            commands.entity(child).insert((
                aabb,
                ActorMeshJointBounds {
                    skin_slot,
                    joint_aabbs: Arc::clone(&built.joint_aabbs),
                },
                NoAutoAabb,
            ));
        }
    }

    instance_slots
}

// Cost/benefit tuning, not a derived value: at 50m (50° vFOV) a character's
// ground shadow is a foreshortened ~20px smudge, but its submeshes still cost
// full draw-call encode in every shadow cascade — a populated Jeuno at Ultra
// measured 33fps from exactly that (kuluu-06jb). The hysteresis band keeps
// actors at the boundary from thrashing archetype moves every frame.
const CHARACTER_SHADOW_CAST_MAX_DISTANCE: f32 = 50.0;
const CHARACTER_SHADOW_CAST_HYSTERESIS: f32 = 5.0;

fn shadow_cast_wanted(cast_enabled: bool, currently_blocked: bool, dist_to_camera: f32) -> bool {
    if !cast_enabled {
        return false;
    }
    let threshold = if currently_blocked {
        CHARACTER_SHADOW_CAST_MAX_DISTANCE - CHARACTER_SHADOW_CAST_HYSTERESIS
    } else {
        CHARACTER_SHADOW_CAST_MAX_DISTANCE + CHARACTER_SHADOW_CAST_HYSTERESIS
    };
    dist_to_camera < threshold
}

// Runs every frame: the camera moves, so an actor's cast/no-cast state can flip
// without any settings change. Commands are only issued on state flips.
pub(crate) fn apply_character_shadow_cast(
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut commands: Commands,
    q_cam: Query<&GlobalTransform, With<crate::camera::OperatorCamera>>,
    q_all: Query<
        (Entity, &GlobalTransform, Has<bevy::light::NotShadowCaster>),
        With<FfxiActorMeshChild>,
    >,
) {
    let cam_pos = q_cam.iter().next().map(|t| t.translation());
    for (e, tf, blocked) in &q_all {
        let want_cast = cam_pos.is_some_and(|cam| {
            shadow_cast_wanted(
                settings.character_shadow_cast,
                blocked,
                tf.translation().distance(cam),
            )
        });
        if want_cast == blocked {
            let mut ec = commands.entity(e);
            if want_cast {
                ec.remove::<bevy::light::NotShadowCaster>();
            } else {
                ec.insert(bevy::light::NotShadowCaster);
            }
        }
    }
}

// Mirrors bevy_camera-0.19.0/src/visibility/mod.rs:594-625
// (update_skinned_mesh_bounds), which cannot see the MeshTag/storage-buffer
// skinning path; registered in VisibilitySystems::CalculateBounds so
// CheckVisibility frustum-culls posed actors instead of drawing every submesh.
pub(crate) fn update_actor_mesh_aabbs(
    registry: Res<FfxiSkinRegistry>,
    mut q: Query<(&mut Aabb, &ActorMeshJointBounds)>,
) {
    q.par_iter_mut().for_each(|(mut aabb, bounds)| {
        if let Some(next) =
            entity_aabb_from_joints(&registry.skin(bounds.skin_slot).joints, &bounds.joint_aabbs)
        {
            *aabb = next;
        }
    });
}

// `pub` so the offline harnesses (and the rabbit tester's integration test) can build a render
// actor straight from a LoadedActor without the mesh/material pipeline.
pub fn make_render_actor(
    loaded: &LoadedActor,
    skin_slot: u32,
    instance_slots: Vec<u32>,
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
        action_assets: Arc::clone(&loaded.action_assets),
        rejected_clips: loaded.rejected_clips.clone(),
        model_dat: loaded.model_dat.clone(),
        coordinator: SkeletonAnimationCoordinator::new(),
        skin_slot,
        instance_slots,
        inputs: ActorAnimInputs::default(),
        world_id,
        facing_dir,
        scale,
        movement_type: loaded
            .cib
            .map(|c| c.movement_type)
            .unwrap_or(MovementType::Unset),
        current_clip: None,
        rest_phase: RestPlayback::Inactive,
        death_phase: actor_state::DeathPhase::Unobserved,
        engage: EngageMachine::NotEngaged,
        action: None,
        action_clips: Vec::new(),
        head_neck,
        head_subtree,
        head_rot: Quat::IDENTITY,
        last_clip: None,
        last_frame: 0.0,
        world_pose: Vec::new(),
        pose_work: PoseScratch::default(),
        point_light_selection: None,
    }
}

// A posed actor with no model behind it, for tests that need the pose/skeleton pair a particle
// attachment reads and nothing else.
#[cfg(test)]
pub(crate) fn render_actor_for_test(skeleton: Skeleton, world_pose: Vec<Mat4>) -> FfxiRenderActor {
    let loaded = LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes: Vec::new(),
        effect_meshes: Vec::new(),
        textures: Vec::new(),
        animations: Arc::default(),
        battle_clips: Arc::default(),
        routines: Arc::default(),
        action_assets: Arc::default(),
        rejected_clips: Vec::new(),
        model_dat: String::new(),
        cib: None,
    };
    FfxiRenderActor {
        world_pose,
        ..make_render_actor(&loaded, 0, Vec::new(), 0, 0.0, 1.0)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_live_actor(
    commands: &mut Commands,
    mesh_handles: &[Handle<Mesh>],
    materials: &mut Assets<FfxiSkinnedMaterial>,
    material_cache: &mut FfxiSkinnedMaterialCache,
    registry: &mut FfxiSkinRegistry,
    images: &mut Assets<Image>,
    prepared: &PreparedActor,
    wire_entity: Entity,
    world_id: u32,
    scale: f32,
) -> Entity {
    commands
        .entity(wire_entity)
        .insert(crate::scene::NameplateLocator::from_skeleton(
            &prepared.loaded.skeleton,
            scale,
        ));
    let facing_dir = 0.0;

    let skin_slot = registry.alloc_skin();
    registry.skin_mut(skin_slot).joints = prepared.parts.bind_joints.clone();

    let actor_root = commands
        .spawn((
            Transform {
                translation: Vec3::ZERO,
                rotation: ffxi_to_bevy_basis(),
                scale: Vec3::ONE,
            },
            GlobalTransform::default(),
            Visibility::default(),
            FfxiSkinSlot(skin_slot),
            ChildOf(wire_entity),
        ))
        .id();

    let instance_slots = build_actor_children(
        commands,
        mesh_handles,
        materials,
        material_cache,
        registry,
        images,
        &prepared.loaded,
        &prepared.parts,
        actor_root,
        skin_slot,
    );

    commands.entity(actor_root).insert(make_render_actor(
        &prepared.loaded,
        skin_slot,
        instance_slots,
        world_id,
        facing_dir,
        scale,
    ));
    insert_auto_run_effects(commands, actor_root, &prepared.loaded);

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

/// Pose one actor outside the live snapshot path, for the offline render
/// harnesses. `mount` is what `tick_live_ffxi_actors` derives from the mount
/// actor's own pose; passing it here keeps the seat maths in one place.
pub fn advance_actor_pose_standalone(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    mount: Option<MountAttach>,
) {
    advance_actor_pose(actor, elapsed_frames, None, mount, false);
}

pub fn tick_ffxi_render_actors(
    time: Res<Time>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut q_actors: Query<&mut FfxiRenderActor>,
) {
    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    q_actors.par_iter_mut().for_each(|mut actor| {
        advance_actor_pose(&mut actor, elapsed_frames, None, None, false);
    });
    for actor in &q_actors {
        registry
            .skin_mut(actor.skin_slot)
            .joints
            .set_from(&actor.world_pose);
    }
}

// Direct resolution of a requested clip id against the available sets: overlay first, then
// primary, deduped by exact chunk id. No fallback here; an empty result is information (the
// model ships no usable chunk for this parameterized id), and the caller decides what to do
// with it.
fn pose_clip_matches<'a>(
    primary: &'a [SkeletonAnimation],
    overlay: impl Iterator<Item = &'a SkeletonAnimation> + Clone,
    id: DatId,
) -> Vec<&'a SkeletonAnimation> {
    let mut seen: std::collections::HashSet<DatId> = std::collections::HashSet::new();
    overlay
        .clone()
        .chain(primary.iter())
        .filter(|a| a.id.parameterized_match(&id) && seen.insert(a.id))
        .collect()
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

/// The routine's *last* Motion stage. Every `fsh<n>` routine carries two — a
/// wind-up and the pose it settles into (`fsh0` = `fh0?` cast then `fh1?` wait,
/// `fsh1` = `fh8?` set-the-hook then `fh2?` fight). A phase the client holds for
/// an indefinite time has to loop the settled stage; looping the wind-up instead
/// replays the cast over and over.
fn routine_motion_clip_last(routines: &HashMap<DatId, Scheduler>, routine: DatId) -> Option<DatId> {
    let sched = routines.get(&routine)?;
    sched
        .stages
        .iter()
        .rfind(|t| t.stage.kind == StageKind::Motion)
        .map(|t| DatId::from_name(&t.stage.id))
}

/// The `ded?` collapse clip and the routine-authored frames retail plays it for
/// before swapping to the held `cor?` corpse pose. Both are Motion stages of the
/// `dead` routine (`dat-routine-stages 7072 dead`: `ded?` dur=116 half-frames at
/// frame 0, `cor?` at frame 116), and the duration is per race - 68..156
/// half-frames across the seven PC skeletons - so it is read, never assumed.
fn death_collapse_clip(routines: &HashMap<DatId, Scheduler>) -> Option<(DatId, f32)> {
    let sched = routines.get(&actor_state::death_routine_id())?;
    let mut motions = sched
        .stages
        .iter()
        .filter(|t| t.stage.kind == StageKind::Motion);
    let collapse = motions.next()?;
    // A routine with a single Motion stage has no corpse pose to fall through to;
    // treat it as having no collapse rather than holding `ded?` forever.
    motions.next()?;
    Some((
        DatId::from_name(&collapse.stage.id),
        half_frames(collapse.stage.duration_frames),
    ))
}

pub(crate) use ffxi_vocab::magic::CATEGORY_MAGIC_START as MAGIC_START_CATEGORY;

// vendor/server/src/map/enums/four_cc.h FourCC - SkillUse/ItemUse/RangedStart carry the
// routine's FourCC in BATTLE2 cmd_arg ("cate"/"cait"/"calg"); "sp??" is that category's
// interrupt. A payload that is not such a FourCC falls back to the category's hard-coded retail
// default. Category 8 keeps its spell-table suffix path for unknown payloads.
pub(crate) fn action_routine(
    action_kind: u8,
    cmd_arg: u32,
    cast_suffix: Option<&str>,
    animation: Option<u16>,
) -> Option<(DatId, bool)> {
    let fourcc = ffxi_vocab::magic::magic_start_routine(cmd_arg);
    Some(match action_kind {
        // BATTLE2's per-result `animation` picks the limb routine
        // (vendor/server/src/map/attack.h AttackAnimation): RightAttack→ati0 / LeftAttack→bti0 /
        // RightKick→cti0 / LeftKick→dti0. Absent or out-of-range values fall back to ati0, the
        // only swing every armed race base is known to carry.
        1 => (
            animation
                .and_then(ffxi_proto::melee::AttackAnimation::from_wire)
                .and_then(crate::scheduler_runtime::swing_routine)
                .map(|name| DatId::from_name(&name))
                .unwrap_or_else(|| DatId::from_str("ati0")),
            false,
        ),

        7 | 9 | 10 | 12 => match fourcc.as_ref().filter(|m| !m.interrupt) {
            // A valid "ca??" start keeps its category's looping semantics: the generic
            // `cast`/`calg` holds loop until resolution, while `cate`/`cait` play once.
            Some(m) => (DatId::from_name(&m.id), matches!(action_kind, 10 | 12)),
            None => match action_kind {
                7 => (DatId::from_str("cate"), false),
                9 => (DatId::from_str("cait"), false),
                10 => (DatId::from_str("cast"), true),
                _ => (DatId::from_str("calg"), true),
            },
        },

        MAGIC_START_CATEGORY => {
            let id = cast_suffix
                .map(|s| DatId::from_str(&format!("ca{s}")))
                .unwrap_or_else(|| DatId::from_str("cast"));
            (id, true)
        }

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

// The precedence tier that owns a frame's pose selection. CLIP_WARN names the tier so a miss
// on an override (special/fishing asking for a clip the model does not ship) is distinguishable
// from a miss on the base tiers.
#[derive(Clone, Copy, PartialEq)]
enum PoseTier {
    Death,
    Action,
    EngageOverlay,
    Fishing,
    Special,
    Rest,
    Locomotion,
}

// `animation_locked` is always false here: the action was just cleared above, so no lock can
// be in effect on the re-pose.
fn reset_actor_pose_state(actor: &mut FfxiRenderActor, elapsed_frames: f32) {
    actor.inputs = ActorAnimInputs::default();
    actor.rest_phase = RestPlayback::Inactive;

    actor.action = None;
    actor.engage = EngageMachine::NotEngaged;
    actor.coordinator.clear();
    actor.current_clip = None;
    advance_actor_pose(actor, elapsed_frames, None, None, false);
    // Ordered after the re-pose, whose default (alive) inputs would otherwise read
    // as having watched this actor alive: retail only plays `ded?` for a death it
    // saw, so a KO'd zone-in resumes on the held corpse frame.
    actor.death_phase = actor_state::DeathPhase::Unobserved;
}

// Runs inside the parallel per-actor pass: it touches only the actor's own
// fields, leaving the pose in `world_pose` for the serial registry copy.
fn advance_actor_pose(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    look: Option<(Mat4, Vec3)>,
    mount: Option<MountAttach>,
    animation_locked: bool,
) {
    let FfxiRenderActor {
        skeleton,
        animations,
        battle_clips,
        routines,
        coordinator,
        inputs,
        facing_dir,
        scale,
        current_clip,
        rest_phase,
        death_phase,
        engage,
        action,
        action_clips,
        head_neck,
        head_subtree,
        head_rot,
        last_clip,
        last_frame,
        world_pose,
        pose_work,
        rejected_clips,
        model_dat,
        ..
    } = actor;
    let animations: &[SkeletonAnimation] = animations;
    let battle_clips: &[SkeletonAnimation] = battle_clips;

    let action_id = match action.as_mut() {
        Some(act) => {
            act.remaining -= elapsed_frames;
            // While an AnimationLock is held, the routine's Motion stage owns the clip: keep it
            // selected past its own length instead of releasing to idle. One-shots pin their end
            // frame in the coordinator (num_loops = 1), so this is what holds a buried/emerged
            // pose until the lock lapses - the general form of the old burrow_holding flag.
            if act.remaining <= 0.0 && !animation_locked {
                *action = None;
                action_clips.clear();
                None
            } else {
                Some(act.clip_id)
            }
        }
        None => None,
    };

    let engage_overlay = match *engage {
        EngageMachine::Drawing { .. } => routine_motion_clip(routines, DatId::from_str("in 0")),
        EngageMachine::Sheathing { .. } => routine_motion_clip(routines, DatId::from_str("out0")),
        _ => None,
    };

    // research/xim Actor.kt (updateFishingState) — the fishing macro-pose overrides
    // locomotion/idle/rest. fsh0 (cast/wait) and fsh1 (fighting) loop; fsh2..fsh6
    // (resolution) play once and hold (see the one-shot handling below).
    //
    // `fsh<n>` is a routine, not a clip: retail enqueues it as a model routine and
    // the routine's first Motion stage names the real `fh<n>?` animation. Looking
    // the routine id up in `animations` directly matches nothing, which is why the
    // pose never played.
    let fishing = inputs
        .fishing_phase
        .and_then(actor_state::fishing_clip)
        .and_then(|fc| {
            // A looping phase (cast/wait, fighting) is held for an indefinite
            // time, so it settles on the routine's last Motion stage; a
            // resolution phase plays its wind-up once and holds.
            let motion = if fc.looping {
                routine_motion_clip_last(routines, fc.id)
            } else {
                routine_motion_clip(routines, fc.id)
            };
            motion.map(|id| actor_state::FishingClip {
                id,
                looping: fc.looping,
            })
        });

    // Special-pose override: the wire's animationsub names a routine
    // (init/ini1/ini2/ini3) and retail plays it on this model through its named-play slots; the
    // resolver walks the model DAT, so a name the model does not ship is a no-op. The pose pass
    // asks for that routine's first Motion stage clip; models without the routine (or without a
    // usable chunk for it) fall through to locomotion like any other miss. No per-mob
    // interpretation: what the sub value does on this model is defined by its DAT alone.
    let special_clip_id = inputs.special.active_routine.and_then(|name| {
        let routine = DatId::from_name(&name);
        let clip = routine_motion_clip(routines, routine);
        if clip.is_none() {
            clip_warn_once(actor.world_id, model_dat, &routine, "routine_not_found");
        }
        clip
    });

    // Retail's `dead` routine outranks locomotion and any in-flight action, so the collapse
    // heads the selection chain; when its timer expires the held `cor?` takes over through the
    // locomotion tier.
    let dead = actor_state::corpse_pose_selected(inputs);
    // A missing collapse clip must not stall the corpse in Collapsing behind a pose that never
    // draws: filter to clips this model ships, so a dead actor without `ded?` settles straight
    // to the held `cor?`.
    let collapse = dead
        .then(|| death_collapse_clip(routines))
        .flatten()
        .filter(|(id, _)| {
            animations
                .iter()
                .any(|clip| clip.id.parameterized_match(id))
        });
    *death_phase = actor_state::next_death_phase(
        *death_phase,
        dead,
        collapse.map_or(0.0, |(_, frames)| frames),
        elapsed_frames,
    );
    let collapse_id = match *death_phase {
        actor_state::DeathPhase::Collapsing { .. } => collapse.map(|(id, _)| id),
        _ => None,
    };

    let use_battle = action.is_some()
        || !matches!(*engage, EngageMachine::NotEngaged)
        || inputs.engage_state.is_battle_idle();
    let overlay: &[SkeletonAnimation] = if use_battle { battle_clips } else { &[] };
    // Skill-DAT (localDir) clips win over the actor's own pose set, per XIM resolution order.
    let resolve = |id: DatId| -> Vec<&SkeletonAnimation> {
        if !action_clips.is_empty() {
            pose_clip_matches(animations, action_clips.iter().chain(overlay.iter()), id)
        } else {
            pose_clip_matches(animations, overlay.iter(), id)
        }
    };
    let usable = |matches: &[&SkeletonAnimation]| matches.iter().any(|a| is_usable_clip(a));

    // Walk the precedence tiers in order (death collapse > action > engage overlay > fishing >
    // special > rest > locomotion). A tier claims the pose only when its clip resolves to at
    // least one usable chunk in this model's sets; a requested clip the model does not ship warns
    // once and falls through
    // to the next tier instead of pinning current_clip. That is what keeps an entity whose named
    // routine the model does not ship out of the special override: its clip never resolves, so
    // selection drops to locomotion and the walk/idle clip loops normally rather than registering
    // as a one-shot that holds its end frame. No per-mob or pool gating decides this; only what
    // the DAT resolves does.
    let mut one_shot_rest = false;
    let try_tier = |id: DatId, is_idle: bool, tier: PoseTier| {
        if usable(&resolve(id)) {
            Some((id, is_idle, tier))
        } else {
            clip_miss(actor.world_id, model_dat, &id, rejected_clips, tier);
            None
        }
    };
    let chosen = collapse_id
        .and_then(|id| try_tier(id, false, PoseTier::Death))
        .or_else(|| action_id.and_then(|id| try_tier(id, false, PoseTier::Action)))
        .or_else(|| engage_overlay.and_then(|id| try_tier(id, false, PoseTier::EngageOverlay)))
        .or_else(|| fishing.and_then(|fc| try_tier(fc.id, fc.looping, PoseTier::Fishing)))
        .or_else(|| special_clip_id.and_then(|id| try_tier(id, false, PoseTier::Special)))
        .or_else(|| {
            // The rest state machine advances only once selection reaches the Rest tier; a
            // higher-priority override that resolved above leaves it untouched. Only the middle
            // phase loops; In/Out are one-shots that hold their last frame (looping them replays
            // the kneel from frame 0 as the character finishes standing up).
            let id = advance_rest_phase(rest_phase, inputs.rest, animations, elapsed_frames)?;
            let looping = matches!(rest_phase, RestPlayback::Looping { .. });
            let chosen = try_tier(id, looping, PoseTier::Rest)?;
            one_shot_rest = !looping;
            Some(chosen)
        })
        .or_else(|| {
            let s = actor_state::selected_animation(inputs);
            try_tier(s.id, s.idle, PoseTier::Locomotion)
        });

    // Terminal fallback: even the lowest tier resolved to nothing, so fall back to the idle family
    // (retail's behavior). If that too is empty there is genuinely no usable clip in this model and
    // current_clip stays untouched; that is the frozen-mob signature CLIP_WARN reports.
    let (selected_id, is_idle, selected_tier) = match chosen {
        Some(c) => c,
        None => {
            clip_warn_once(
                actor.world_id,
                model_dat,
                &DatId::from_str("idl?"),
                "not_found",
            );
            (DatId::from_str("idl?"), true, PoseTier::Locomotion)
        }
    };

    // A chosen tier always resolved to a usable chunk above, so this is non-empty unless we fell
    // through every tier and the idle family itself is missing from the model.
    let matches: Vec<&SkeletonAnimation> = resolve(selected_id);
    if matches.is_empty() {
        clip_warn_once(
            actor.world_id,
            model_dat,
            &selected_id,
            "no_match_kept_previous",
        );
    }

    // Gated special-pose diagnostics: log every pose-selection change that touches the active
    // routine's clip or happens while a special state is up. Catches a silent fall-through (the
    // model ships no usable chunk for the named routine) and a higher-priority override releasing
    // the one-shot before INVISIBLE arrives.
    if special_log_enabled() && !matches.is_empty() {
        let changed = *current_clip != Some((selected_id, use_battle));
        if changed {
            let touches_special = inputs.special.active_routine.is_some()
                || special_clip_id.is_some_and(|sc| {
                    selected_id.parameterized_match(&sc)
                        || current_clip.is_some_and(|(id, _)| id.parameterized_match(&sc))
                });
            if touches_special {
                tracing::info!(
                    target: "special",
                    id = actor.world_id,
                    ?inputs.special,
                    selected = %selected_id.as_str(),
                    use_battle,
                    matches_count = matches.len(),
                    "pose-select"
                );
            }
        }
    }

    // The coordinator cursor resets ONLY when the selected clip actually changes (wlk<->run,
    // moving<->idle, or a tier/clip-set change such as casual->battle on engage). A new POS update
    // within the same gait is NOT a clip change: it must not re-register here or call
    // register_animation again, or the walk clip would restart from frame 0 on every packet (the
    // stop-and-go stutter of B). The chase model keeps `moving` up across a late update via its
    // hold-until grace window (combat_stance::PredictSample), so an unchanged gait never reaches this
    // branch. There is no per-frame or per-update re-registration path for an unchanged locomotion
    // clip: coordinator.update below advances the cursor monotonically instead.
    if !matches.is_empty() && *current_clip != Some((selected_id, use_battle)) {
        // RUST_LOG=clip=debug: the requested clip actually resolved; one line per transition into
        // current_clip, naming the chunk that won and its frame count.
        if let Some(resolved) = matches.iter().find(|a| is_usable_clip(a)) {
            clip_ok(actor.world_id, &selected_id, resolved, actor.movement_type);
        }
        *current_clip = Some((selected_id, use_battle));

        let mut new_mask = 0u8;
        for clip in &matches {
            let slot = (clip.id.final_digit().unwrap_or(0) as usize).min(7);
            new_mask |= 1 << slot;
        }

        let old_mask = coordinator.occupied_slots();
        for slot in 0..8usize {
            if old_mask & (1 << slot) != 0 && new_mask & (1 << slot) == 0 {
                coordinator.clear_slot(slot);
            }
        }

        if is_idle {
            for &clip in &matches {
                coordinator.register_idle_animation(clip.clone(), true);
            }
        } else {
            // research/xim EffectRoutineInterpolatedEffects.kt SkeletonAnimationInstance loopParams — when the pose came from
            // a completion motion, honor its parsed transition + loop params; otherwise use the
            // locomotion crossfade defaults.
            let action = action.filter(|a| a.clip_id == selected_id);
            let tp = TransitionParams {
                transition_in_time: action.map_or(LOCOMOTION_XFADE_IN, |a| a.transition_in),
                transition_out_time: action.map_or(LOCOMOTION_XFADE_OUT, |a| a.transition_out),
                ..Default::default()
            };
            // Fishing resolution clips (fsh2..fsh6) have no ActionPlayback, so without an
            // explicit single loop they would default to looping forever; they must play once and
            // hold the final frame until the server advances the state. Only when the fishing tier
            // actually won: a fall-through from it plays a lower-tier clip that loops normally.
            let one_shot_fishing = matches!(selected_tier, PoseTier::Fishing)
                && matches!(fishing, Some(fc) if !fc.looping);
            // Special-pose and death-collapse clips are always one-shots (the routine's motion
            // clip holds its end frame until the wire state settles). Keyed on the winning tier so
            // an entity whose named routine does not resolve loops its locomotion clip instead of
            // pinning it.
            let loop_params = LoopParams {
                loop_duration: None,
                num_loops: action.and_then(|a| a.num_loops).or((one_shot_fishing
                    || one_shot_rest
                    || matches!(selected_tier, PoseTier::Death)
                    || matches!(selected_tier, PoseTier::Special))
                .then_some(1)),
                low_priority: false,
            };
            for &clip in &matches {
                coordinator
                    .register_animation(clip.clone(), loop_params, Some(tp.clone()), |_| true);
            }
        }
    }

    *last_clip = matches
        .iter()
        .max_by_key(|a| a.key_frame_sets.len())
        .map(|a| a.id);

    // Retail's AnimationSpeed (SpeedBase * 0.1) scales walk/run clip playback relative to the
    // authored rate; idle and override tiers play at their authored pace, so only a winning
    // non-idle locomotion tier takes the scale.
    let frame_step = if matches!(selected_tier, PoseTier::Locomotion) && !is_idle {
        elapsed_frames * inputs.playback_rate
    } else {
        elapsed_frames
    };
    coordinator.update(frame_step);

    *last_frame = coordinator
        .animations
        .iter()
        .flatten()
        .filter_map(|a| a.current_animation.as_ref().map(|c| c.current_frame))
        .next_back()
        .unwrap_or(0.0);

    // Gated hold probe: while a special state is up, sample the pinned frame every 30 ticks so a
    // released one-shot shows up as last_frame drifting back toward 0. `done` reports whether the
    // routine's motion clip has played to its end frame; pinning (num_loops = 1) must keep it on
    // that frame until the wire state settles. Retail never hides a model on clip completion,
    // so any drift here is a bug, not something a visibility flag papers over.
    if special_log_enabled()
        && inputs.special.active_routine.is_some()
        && SPECIAL_LOG_TICK
            .load(std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(30)
    {
        let done = special_clip_id.is_some_and(|id| {
            coordinator.animations.iter().flatten().any(|a| {
                a.current_animation
                    .as_ref()
                    .is_some_and(|c| c.animation.id.parameterized_match(&id) && c.is_done_looping())
            })
        });
        tracing::info!(
            target: "special",
            id = actor.world_id,
            ?inputs.special,
            selected = %selected_id.as_str(),
            last_frame,
            done,
            transitioning = coordinator.is_transitioning(),
            "hold-probe"
        );
    }

    pose_world_mounted_into(
        world_pose,
        pose_work,
        skeleton,
        |joint| coordinator.get_joint_transform(joint),
        RootTransform {
            facing_dir: *facing_dir,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::splat(*scale),
        },
        &[],
        mount,
    );

    if let Some(neck) = *head_neck {
        let neck_pose = world_pose
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
        *head_rot = head_rot.slerp(desired, alpha);
        apply_head_look(world_pose, neck, head_subtree, *head_rot);
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
mod shadow_cast_scope_tests {
    use super::*;

    const MAX: f32 = CHARACTER_SHADOW_CAST_MAX_DISTANCE;
    const HYST: f32 = CHARACTER_SHADOW_CAST_HYSTERESIS;

    #[test]
    fn disabled_never_casts() {
        assert!(!shadow_cast_wanted(false, false, 0.0));
        assert!(!shadow_cast_wanted(false, true, 0.0));
    }

    #[test]
    fn near_casts_far_does_not() {
        assert!(shadow_cast_wanted(true, true, MAX - HYST - 1.0));
        assert!(!shadow_cast_wanted(true, false, MAX + HYST + 1.0));
    }

    #[test]
    fn hysteresis_band_preserves_current_state() {
        let in_band = MAX;
        assert!(shadow_cast_wanted(true, false, in_band));
        assert!(!shadow_cast_wanted(true, true, in_band));
    }
}

#[cfg(test)]
mod morph_column_tests {
    use super::*;

    fn morph(orb: Entity, root: Entity) -> crate::components::MorphIn {
        crate::components::MorphIn {
            elapsed: 0.0,
            actor_root: root,
            orb: Some(orb),
            orb_mat: None,
            orb_emissive: LinearRgba::BLACK,
        }
    }

    /// The actual regression: an actor reload replaces `MorphIn` rather than
    /// removing it, and the replacement cannot know the old column's id. Before
    /// the observer owned that lifetime, the column stayed on the player for the
    /// rest of the session.
    #[test]
    fn replacing_the_component_takes_the_old_column_with_it() {
        let mut app = App::new();
        app.add_observer(despawn_morph_column);

        let actor = app.world_mut().spawn_empty().id();
        let wire = app.world_mut().spawn_empty().id();
        let first = app.world_mut().spawn(ChildOf(wire)).id();
        app.world_mut().entity_mut(wire).insert(morph(first, actor));

        let second = app.world_mut().spawn(ChildOf(wire)).id();
        app.world_mut()
            .entity_mut(wire)
            .insert(morph(second, actor));
        app.update();

        assert!(app.world().get_entity(first).is_err(), "old column leaked");
        assert!(app.world().get_entity(second).is_ok(), "new column reaped");
    }

    #[test]
    fn finishing_the_morph_takes_its_column_with_it() {
        let mut app = App::new();
        app.add_observer(despawn_morph_column);

        let actor = app.world_mut().spawn_empty().id();
        let wire = app.world_mut().spawn_empty().id();
        let orb = app.world_mut().spawn(ChildOf(wire)).id();
        app.world_mut().entity_mut(wire).insert(morph(orb, actor));

        app.world_mut()
            .entity_mut(wire)
            .remove::<crate::components::MorphIn>();
        app.update();

        assert!(app.world().get_entity(orb).is_err());
    }
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
    ready: std::collections::VecDeque<(u32, Option<ActorPrepKey>, Arc<PreparedActor>)>,
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
            in_flight.ready.retain(|(id, _, _)| *id != req.entity_id);
            in_flight
                .ready
                .push_back((req.entity_id, Some(key), prepared));
            continue;
        }
        let subject = req.subject.clone();
        // Retail applies the 0x45 Info `scale` byte to NPC models only: NpcModel.getScale
        // divides it by 100 (research/xim poc/Model.kt NpcModel.getScale), PcModel drops the
        // byte entirely (PcModel.getMovementInfo) and the mount layout has no scale field at all.
        // The same value
        // bakes the bind pose/bounds here and rides along to spawn_live_actor's per-frame
        // RootTransform, so both see one number.
        let is_npc = matches!(subject, ActorSubject::Npc { .. });
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let loaded = match subject {
                ActorSubject::Mount { race } => load_mount_race(race),
                ActorSubject::Npc { file_id } => load_npc(file_id),
                ActorSubject::Pc {
                    race,
                    mounted,
                    equipment,
                    body,
                    main_weapon,
                    sub_weapon,
                } => load_pc(race, mounted, &equipment, body, main_weapon, sub_weapon),
            }?;
            let scale = if is_npc {
                loaded.cib.map_or(1.0, |c| c.scale_factor())
            } else {
                1.0
            };
            let parts = prepare_actor_parts(&loaded, 0.0, scale, quality);
            Ok(PreparedActor {
                loaded,
                parts,
                scale,
            })
        });
        // Newest look wins: replacing the entry drops any stale in-flight load.
        in_flight.tasks.insert(req.entity_id, task);
        in_flight.keys.insert(req.entity_id, key);
        in_flight.ready.retain(|(id, _, _)| *id != req.entity_id);
    }
}

pub fn poll_load_actor_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut material_cache: ResMut<FfxiSkinnedMaterialCache>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    tracked: Res<crate::scene::TrackedEntities>,
    entity_mesh: Option<Res<crate::scene::EntityMesh>>,
    mut in_flight: ResMut<ActorLoadInFlight>,
    q_existing: Query<&FfxiRenderRoot>,
    q_ball: Query<&MeshMaterial3d<StandardMaterial>, With<Mesh3d>>,
    state: Res<crate::snapshot::SceneState>,
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
                if let Some(key) = &key {
                    in_flight.cache.insert(key.clone(), Arc::clone(&p));
                }
                in_flight.ready.retain(|(id, _, _)| *id != entity_id);
                in_flight.ready.push_back((entity_id, key, p));
            }
            Err(e) => {
                warn!("ffxi actor load failed (entity {entity_id}): {e}");
            }
        }
    }
    for _ in 0..ACTOR_SPAWNS_PER_FRAME {
        let Some((entity_id, key, prepared)) = in_flight.ready.pop_front() else {
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

        let mesh_handles = key
            .as_ref()
            .and_then(|k| in_flight.cache.mesh_handles(k, &mut meshes))
            .unwrap_or_else(|| add_part_meshes(&prepared.parts, &mut meshes));

        let root = spawn_live_actor(
            &mut commands,
            &mesh_handles,
            &mut materials,
            &mut material_cache,
            &mut registry,
            &mut images,
            &prepared,
            wire_entity,
            entity_id,
            prepared.scale,
        );

        // A transient child carries the stretch: the wire entity is driven by
        // sync and the model shares its transform, so neither can be reshaped.
        // A reload has no resting orb to consume and just regrows the model.
        //
        // The placeholder ball is the stand-in for a body that has not loaded
        // yet, so the model arriving consumes it either way. What retail skips
        // for self is only the morph-in column it would have stretched into
        // (retail capture, Upper Jeuno -> Rolanberry Fields, 2026-08-04) —
        // skipping the whole branch here stranded the ball on the player.
        let resting_ball = q_ball.get(wire_entity).ok();
        if resting_ball.is_some() {
            commands.entity(wire_entity).remove::<Mesh3d>();
        }
        let orb = (Some(entity_id) != state.snapshot.self_char_id)
            .then_some(resting_ball)
            .flatten()
            .and_then(|mm| {
                let lit = std_materials.get(&mm.0).map(|m| {
                    let mut m = m.clone();
                    m.alpha_mode = AlphaMode::Blend;
                    m
                })?;
                let emissive = lit.emissive;
                let handle = std_materials.add(lit);
                let orb = commands
                    .spawn((
                        Mesh3d(entity_mesh.morph_orb.clone()),
                        MeshMaterial3d(handle.clone()),
                        Transform::from_xyz(0.0, MORPH_COLUMN_PIVOT_Y, 0.0),
                        // Inherited (not Visible): an INVISIBLE entity's root is
                        // Hidden and must not leak the morph column through it —
                        // Visible would override the parent hide.
                        Visibility::Inherited,
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

        // Animated mob extents need their own camera/picking policy; locator metadata is independent.
        if !matches!(key.as_ref(), Some(ActorPrepKey::Npc { .. })) {
            if let Some((lo, hi)) = prepared.parts.bounds {
                commands.entity(wire_entity).insert(BakedActor {
                    min_mesh_y: lo.y,
                    actor_height: (hi.y - lo.y).max(0.1),
                });
            }
        }
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
            if let Some(mut mat) = std_materials.get_mut(handle) {
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
            commands
                .entity(wire_entity)
                .remove::<crate::components::MorphIn>();
        }
    }
}

/// The column is owned by the [`MorphIn`] that spawned it, not by whoever
/// happens to finish it. Nothing else holds the child's id, so an actor reload
/// — which *replaces* the component rather than removing it — used to strand
/// the old column on a living player forever.
///
/// [`MorphIn`]: crate::components::MorphIn
pub fn despawn_morph_column(
    trigger: On<Discard, crate::components::MorphIn>,
    q: Query<&crate::components::MorphIn>,
    mut commands: Commands,
) {
    let Ok(morph) = q.get(trigger.event().event_target()) else {
        return;
    };
    if let Some(orb) = morph.orb {
        commands.entity(orb).try_despawn();
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

/// Standard-joint index of the seat a rider of `race` occupies on a mount. Mount
/// skeletons carry one per playable race because each sits differently; the block
/// starts at 48 and is indexed by the look race less one.
/// research/xim resource/SkeletonInstance.kt, applyMountAttachTransform.
fn saddle_joint_index(race: u8) -> Option<usize> {
    const SADDLE_JOINT_BASE: usize = 48;
    const PLAYABLE_RACES: u8 = 8;
    (1..=PLAYABLE_RACES)
        .contains(&race)
        .then(|| SADDLE_JOINT_BASE + usize::from(race - 1))
}

/// The nudge xim applies on top of a joint-derived seat, marked in its source as
/// an unexplained fudge (research/xim resource/SkeletonInstance.kt,
/// applyMountAttachTransform).
const SADDLE_JOINT_NUDGE: Vec3 = Vec3::new(0.0, -0.1, 0.0);

/// Base of a chocobo's spine — the stretch of back the saddle is strapped to,
/// and the part of the animal that rises and falls with its gait. Its race
/// skeletons file the pelvis and the first two spine segments as one coincident
/// chain, so any of the three reads the same seat.
const CHOCOBO_BACK_JOINT: usize = 3;

/// How far above that joint the rider's hip belongs. A chocobo declares no seat
/// of its own — the whole per-race saddle block is dead — and retail hard-codes
/// the height too (research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp,
/// `SkeletalMeshActor::GetElem`, a flat 1.3 for `IsOnChocobo`), but against the
/// actor root rather than the animated back, so the magnitude does not
/// transplant. Calibrated against retail footage (Rolanberry Fields,
/// 2026-08-04): the belt clears the saddle and the boot falls level with the
/// chocobo's knee. Skeleton space is Y-down, so up is negative.
const CHOCOBO_SEAT_ABOVE_BACK: f32 = -0.24;

/// Where the rider's hip joint is pinned on the mount it is riding, in the
/// mount's skeleton space. Both actors share a world transform, so the mount's
/// own pose needs no reframing to be read as the rider's.
///
/// A chocobo's seat has to be read off its animated back and not from a fixed
/// height: the back travels about a tenth of a yalm through a gallop, and a
/// rider held still against that has the saddle saw up through their body.
pub fn mount_seat_local(
    mount_pose: &[Mat4],
    mount_skeleton: &Skeleton,
    rider_race: u8,
    chocobo: bool,
) -> Option<Vec3> {
    if chocobo {
        return chocobo_seat_local(mount_pose, CHOCOBO_SEAT_ABOVE_BACK);
    }
    let joint = saddle_joint_index(rider_race)?;
    let seat = standard_joint_world_position(mount_pose, mount_skeleton, joint)?;
    Some(seat + SADDLE_JOINT_NUDGE)
}

/// [`mount_seat_local`]'s chocobo case with the height left open, so the render
/// harness can sweep it against footage without rebuilding the library.
///
/// Only the back's *height* is taken. Its joint sits behind the animal's origin,
/// which is already the middle of the saddle, so carrying x/z across would slide
/// the rider back over the tail.
pub fn chocobo_seat_local(mount_pose: &[Mat4], above_back: f32) -> Option<Vec3> {
    let back = mount_pose
        .get(CHOCOBO_BACK_JOINT)?
        .to_scale_rotation_translation()
        .2;
    Some(Vec3::new(0.0, back.y + above_back, 0.0))
}

// Clone only (not Copy): `name` is a String.
#[derive(Clone)]
pub struct SnapshotActorState {
    pos: kuluu_snapshot::Vec3,
    // Head-look: facetarget is a targid (act_index), so resolve it to the world_id
    // the position maps are keyed by. Distinct from bt_target_id (the combat-claim
    // UniqueNo), which only turns the head mid-combat and lives in a different
    // id-space — see vendor/server char_update.cpp Flags0.facetarget.
    face_target: u16,
    // Engaged combat stance is the server's animation byte (ANIMATION_ATTACK),
    // set on every entity at engage and broadcast in the General block — see LSB
    // CBattleEntity::OnEngage, vendor/server/src/map/entities/baseentity.h. The
    // reactor goal only *predicts* self-engage for snappy feedback before the
    // server echoes, and only some UIs set it, so it can't be the source of truth.
    engaged: bool,
    dead: bool,
    // The server broadcasts the fsh* and /heal//sit states in the entity's
    // animation byte (server_status), the same channel as engage. Self drives
    // its fishing/rest pose from local state instead, so these are consulted
    // only for observed entities.
    fishing_phase: Option<u8>,
    rest: ffxi_actor::actor_state::RestKind,
    /// Set on a mount actor and on the rider sitting on it. Both play `chi?`:
    /// the mount its carrying pose, the rider the matching seat
    /// (research/xim poc/Actor.kt, Actor.getIdleAnimationId).
    mount_or_chocobo: bool,
    /// A mount actor stands where its rider stands and moves when the rider
    /// moves, so its gait is read from the rider's motion, not its own — it has
    /// no entry of its own in the position stream.
    motion_from: Option<u32>,
    /// Look race of the rider, on a mount actor's entry. Every mount skeleton
    /// carries one saddle joint per playable race, because each sits differently.
    rider_race: u8,
    /// Set on a mount actor's entry when it is a ridden chocobo, whose rider is
    /// seated by their animation rather than pinned to a saddle joint.
    mount_is_chocobo: bool,
    /// Special-pose wire state (raw animationsub, hidden flag, last-triggered routine),
    /// advanced from the entity's status/animationsub transitions by
    /// [`ffxi_actor::actor_state::next_special_pose`]. Drives the named-routine clip override in
    /// [`advance_actor_pose`]; models without that routine get plain locomotion.
    special: ffxi_actor::actor_state::SpecialPose,
    /// The 0x0E speed/animationSpeed bytes for entities whose motion is wire-driven (the chase
    /// model's Mob/Pc/Pet/Npc); `None` on the transform-delta fallback path. Gait rule:
    /// run = speed > speed_base (LSB UpdateSpeed multiplies `speed` only). Mount entries carry
    /// their rider's values because a mount actor has no motion of its own.
    wire_gait: Option<(u8, u8)>,
}

/// Per-entity lookups derived from `SceneState.snapshot.entities`, rebuilt only
/// when the snapshot resource actually changes (its dirty flag bypasses change
/// detection on empty poll frames, so `is_changed` is truthful).
#[derive(Default)]
pub struct LiveSnapshotIndex {
    by_id: HashMap<u32, SnapshotActorState>,
    id_by_targid: HashMap<u16, u32>,
}

/// Per-frame scratch maps rebuilt every tick by [`tick_live_ffxi_actors`]. Grouped into one
/// `Local` so the system stays within Bevy's 16-parameter fn-item arity limit. `pub` like
/// [`LiveSnapshotIndex`]: a Local parameter type must be visible to modules that schedule this
/// system with `.before()`/`.after()`.
#[derive(Default)]
pub struct FrameScratch {
    actor_world: HashMap<u32, Vec3>,
    mount_attach: HashMap<u32, MountAttach>,
}

pub fn tick_live_ffxi_actors(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    motion: Res<combat_stance::EntityMotion>,
    mut rest: ResMut<combat_stance::RestStance>,
    walk_mode: Res<combat_stance::WalkMode>,
    self_move: Res<combat_stance::SelfMoveIntent>,
    mut registry: ResMut<FfxiSkinRegistry>,
    target: Res<crate::scene::Target>,
    tracked: Res<crate::scene::TrackedEntities>,
    // Model-root Visibility is written here only for entities with an active special state;
    // every other entity's root stays owned by scene::apply_invis_flag_system (invis-flag PCs).
    // The fourth slot is the Defeated latch: a killing result starts the death path on
    // this frame instead of waiting for the next 0x0E hp_pct.
    mut q_actors: Query<(
        &mut FfxiRenderActor,
        &GlobalTransform,
        &mut Visibility,
        Option<&crate::scheduler_runtime::DeadFromAction>,
    )>,
    mut commands: Commands,

    mut prev_zone: Local<Option<Option<u16>>>,
    mut index: Local<LiveSnapshotIndex>,
    // Per-frame scratch maps (see FrameScratch); one Local instead of two keeps the system
    // within Bevy's 16-parameter fn-item arity limit.
    mut frame_scratch: Local<FrameScratch>,
    // Last-observed special-pose wire state per entity. Persists across frames (a `Local`), so
    // the transition can advance from the previous snapshot's state; rebuilt entries read their
    // prior state here rather than resetting to plain on every change.
    mut special_mem: Local<HashMap<u32, ffxi_actor::actor_state::SpecialPose>>,
    // The entity-level routine vecs: the special-pose routine firing below and the
    // AnimationLock set built before the parallel pass both read through this one query. Sixteen
    // parameters (Bevy's fn-item arity limit); new state goes into FrameScratch, not here.
    q_scheds: Query<(
        &crate::components::WorldEntity,
        &crate::scheduler_runtime::ActiveSchedulers,
    )>,
) {
    use ffxi_actor::actor_state::RestKind;

    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    let self_id = state.snapshot.self_char_id;

    // Special-pose effect routines queued by this frame's wire-state transitions, mirroring
    // retail: a sub change on a live actor plays table[sub] on the model
    // (a worm's `ini1` = Motion sp1? + dirt generators + sound); a hidden -> visible transition
    // is an actor create in retail and runs the load routine `init` (a worm's pop-up: Motion sp0?
    // + dirt generators + sound). We keep one hidden actor instead of destroying/rebuilding it,
    // so both fire on the same entity. Motion stages are suppressed when flattened; the routine
    // motion clips stay owned by the pose pass, so only VFX/sound come from the routine. A
    // sub-clear arriving mid-routine does nothing in retail (the routine finishes), so there is
    // no early-cancel path here.
    let mut special_routines: Vec<(u32, [u8; 4])> = Vec::new();

    if state.is_changed() {
        use ffxi_actor::actor_state::{next_special_pose, SpecialPose};

        index.by_id.clear();
        index.id_by_targid.clear();
        let mut live_ids = std::collections::HashSet::new();
        for e in &state.snapshot.entities {
            live_ids.insert(e.id);
            let mounted = state.snapshot.mount_of(e).is_some();
            // Advance the special-pose wire state from last frame to this snapshot's
            // status/animationsub. A no-op (stays plain) for entities with no sub and a visible
            // status, so it is safe to run for all of them.
            // Absent from last frame's snapshot = no live actor in retail (destroyed on view-range
            // exit or not yet constructed): model it as hidden so the first visible observation
            // takes the resurface path and runs 'init': leaving and re-entering view is a
            // destroy/create that replays the load routine.
            let prev = special_mem.get(&e.id).copied().unwrap_or(SpecialPose {
                hidden: true,
                ..Default::default()
            });
            let step = next_special_pose(&prev, e.status, e.animationsub);
            if let Some(routine) = step.triggered {
                special_routines.push((e.id, routine));
            }
            // A trigger always changes the pose, so a changed-and-interesting state is the whole
            // log condition: plain visible entities with no sub never appear here.
            if special_log_enabled()
                && step.pose != prev
                && (prev.active_routine.is_some()
                    || step.pose.active_routine.is_some()
                    || step.pose.hidden)
            {
                tracing::info!(
                    target: "special",
                    id = e.id,
                    ?prev,
                    new = ?step.pose,
                    triggered = ?step.triggered,
                    status = e.status,
                    sub = e.animationsub,
                    "fsm"
                );
            }
            special_mem.insert(e.id, step.pose);
            index.by_id.insert(
                e.id,
                SnapshotActorState {
                    pos: e.pos,
                    face_target: e.face_target,
                    engaged: e.animation == ffxi_proto::decode::animation::ATTACK,
                    dead: e.hp_pct == Some(0),
                    fishing_phase: ffxi_proto::decode::animation::fishing_phase(e.animation),
                    rest: observed_rest_kind(e.animation),
                    mount_or_chocobo: mounted,
                    motion_from: None,
                    rider_race: 0,
                    mount_is_chocobo: false,
                    special: step.pose,
                    // LSB sends a PC speed 50 over base 50 whether it walks or runs (the walk
                    // toggle rides the 0x00D RunMode bit), so only server-paced kinds read their
                    // gait from the speed bytes; PCs keep the measured-speed fallback.
                    wire_gait: matches!(
                        e.kind,
                        kuluu_snapshot::EntityKind::Mob
                            | kuluu_snapshot::EntityKind::Pet
                            | kuluu_snapshot::EntityKind::Npc
                    )
                    .then(|| (e.speed, e.speed_base)),
                },
            );
            index.id_by_targid.insert(e.act_index, e.id);

            if mounted {
                index.by_id.insert(
                    crate::scene::mount_actor_id(e.id),
                    SnapshotActorState {
                        pos: e.pos,
                        face_target: 0,
                        engaged: false,
                        dead: false,
                        fishing_phase: None,
                        rest: ffxi_actor::actor_state::RestKind::None,
                        mount_or_chocobo: true,
                        motion_from: Some(e.id),
                        rider_race: match e.look {
                            Some(kuluu_snapshot::EntityLook::Equipped { race, .. }) => race,
                            _ => 0,
                        },
                        mount_is_chocobo: state
                            .snapshot
                            .mount_of(e)
                            .is_some_and(|m| m.is_chocobo()),
                        special: ffxi_actor::actor_state::SpecialPose::default(),
                        wire_gait: None,
                    },
                );
            }
        }
        // Drop states for entities that despawned so the cache stays bounded.
        special_mem.retain(|id, _| live_ids.contains(id));
    }

    // Fire the queued routines on their wire entities: it carries a world-space Transform (the
    // particle/sound origin) and is what the stage-dispatch systems read `(Transform,
    // Option<ActionAssets>)` off. The actor's own ActionAssets hold this DAT's SEPs and dirt
    // generators, so they ride along for resolution.
    for (world_id, routine) in special_routines {
        let Some(&wire_e) = tracked.by_id.get(&world_id) else {
            continue;
        };
        // Model not loaded yet: the clip still plays from the pose pass; only the dirt and
        // sound are lost. Acceptable degradation — the load lands within a few frames.
        let Some((actor, _, _, _)) = q_actors.iter().find(|(a, _, _, _)| a.world_id == world_id)
        else {
            continue;
        };
        let lookup = crate::scheduler_runtime::RoutineLookup::new().with_actor(actor.routines());
        let Some(active) =
            crate::scheduler_runtime::ActiveScheduler::effects_only(&lookup, &routine)
        else {
            continue;
        };
        // Insert-or-push like the other dispatchers: a pop-up `init` alongside a still-running
        // dig `ini1` (or vice versa) runs concurrently in retail; each carries the 0x5F that
        // stops the other, so the overlap resolves through StopRoutine. The push path leaves the
        // first writer's ActionAssets/ActionTarget alone.
        crate::scheduler_runtime::enqueue_routine(&mut commands, wire_e, active);
        commands
            .entity(wire_e)
            .try_insert_if_new(actor.action_assets().clone())
            .try_insert_if_new(crate::scheduler_runtime::ActionTarget(None));
    }

    if special_log_enabled() {
        SPECIAL_LOG_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let index: &LiveSnapshotIndex = &index;
    // Split borrows of the scratch fields: each `.field` access through the Local's DerefMut
    // would take its own whole-value mutable borrow, so materialize one plain `&mut`
    // first and borrow disjoint fields from it.
    let frame_scratch = &mut *frame_scratch;
    let actor_world_scratch = &mut frame_scratch.actor_world;
    let mount_attach_scratch = &mut frame_scratch.mount_attach;

    // Head-look must aim at where the target is *rendered* (grounded), not its
    // raw wire Y — the server sends pathing NPCs a flat reference Y, so wire and
    // rendered Y diverge after snap_entities_to_mzb_floor_system.
    actor_world_scratch.clear();
    actor_world_scratch.extend(
        q_actors
            .iter()
            .map(|(a, gt, _, _)| (a.world_id, gt.translation())),
    );
    let actor_world_by_id: &HashMap<u32, Vec3> = actor_world_scratch;

    // Where each rider's body has to be pinned, for the mounts that pin one.
    // Read off the mount actor's posed skeleton, which shares the rider's root
    // transform exactly (scene.rs pins the mount entity to the rider's), so the
    // joint needs no reframing. Taken from the pose the mount held last frame —
    // the two actors are posed in the same pass and a frame of lag on a seat is
    // not visible.
    mount_attach_scratch.clear();
    for (a, _, _, _) in &q_actors {
        let Some(rider_id) = crate::scene::mount_actor_rider(a.world_id) else {
            continue;
        };
        let Some(mount_state) = index.by_id.get(&a.world_id) else {
            continue;
        };
        if let Some(seat) = mount_seat_local(
            &a.world_pose,
            &a.skeleton,
            mount_state.rider_race,
            mount_state.mount_is_chocobo,
        ) {
            mount_attach_scratch.insert(
                rider_id,
                MountAttach {
                    mount_joint_world: seat,
                    // Heading lives on the entity Transform here, not in the pose
                    // frame, so the rider only takes the mount's own seat rotation.
                    facing_dir: 0.0,
                    rider_rotation: 0.0,
                },
            );
        }
    }
    let mount_attach_by_rider: &HashMap<u32, MountAttach> = mount_attach_scratch;

    // World ids whose running routine currently holds an AnimationLock: while locked
    // the pose pass must not release the routine's Motion clip - a one-shot pins its end frame in
    // the coordinator, so this is what holds buried/emerged poses until the lock lapses. Built
    // serially before the parallel pass; the set is read-only inside it.
    // dead_fall_pending: world ids whose Defeated latch is up but the `dead` routine's
    // fall-over Motion has not fired yet: hold idle across that gap instead of flashing cor?.
    let mut animation_locked = std::collections::HashSet::new();
    let mut dead_fall_pending = std::collections::HashSet::new();
    for (world, scheds) in &q_scheds {
        if scheds.is_locked_now() {
            animation_locked.insert(world.id);
        }
        if scheds.dead_fall_over_pending() {
            dead_fall_pending.insert(world.id);
        }
    }

    let self_engaged_predicted = matches!(
        state.snapshot.current_goal,
        Some(kuluu_snapshot::ReactorGoal::Engaged { .. })
    );
    let self_reactor_driven = !matches!(
        state.snapshot.current_goal,
        None | Some(kuluu_snapshot::ReactorGoal::Idle)
    );

    let zone = state.snapshot.zone_id;
    let zone_changed = matches!(*prev_zone, Some(p) if p != zone);
    *prev_zone = Some(zone);

    // Self rest pose comes from local input (RestStance), not the wire byte.
    let self_rest_kind = match rest.kind {
        combat_stance::RestKind::None => RestKind::None,
        combat_stance::RestKind::Sit => RestKind::Sit,
        combat_stance::RestKind::Heal => RestKind::Heal,
    };
    // Self fishing pose comes from the local mini-game machine (it knows the
    // active reeling sub-states the server never broadcasts).
    let self_fishing_phase = state.snapshot.self_fishing.map(|f| f.phase);
    let self_casting = state
        .snapshot
        .self_casting
        .as_ref()
        .is_some_and(|c| !c.interrupted);
    let self_walking = self_move.walking(walk_mode.walking);
    let self_target_id = target.id;
    let (self_move_forward, self_move_strafe, self_move_moving) =
        (self_move.forward, self_move.strafe, self_move.moving);

    // Self KO is unreliable via the entity hp_pct (only updated when CHAR_PC
    // carries UPDATE_HP) and via the party row (absent/stale when solo).
    // death_homepoint_secs is published from 0x037 CHAR_STATUS and 0x00A LOGIN,
    // both gated on hpp == 0.
    let self_dead = state.snapshot.death_homepoint_secs.is_some()
        || crate::snapshot::resolve_self(&state.snapshot.party, self_id)
            .map(|m| m.hp_pct == 0)
            .unwrap_or(false);

    let motion = &*motion;
    q_actors
        .par_iter_mut()
        .for_each(|(mut actor, actor_global, mut vis, dead_from_action)| {
            let world_id = actor.world_id;
            if world_id == 0 {
                return;
            }

            let is_self = Some(world_id) == self_id;
            let snap = index.by_id.get(&world_id);

            if zone_changed || (!is_self && snap.is_none()) {
                reset_actor_pose_state(&mut actor, elapsed_frames);
                return;
            }

            // A mount actor has no motion of its own: it is pinned to its rider's
            // transform, so its gait must come from whatever drives the rider.
            let motion_id = snap.and_then(|s| s.motion_from).unwrap_or(world_id);
            let drives_from_self_input = Some(motion_id) == self_id;
            let sample = motion.sample(motion_id).unwrap_or_default();

            let engaged =
                snap.map(|s| s.engaged).unwrap_or(false) || (is_self && self_engaged_predicted);
            // a Defeated result latches the death path on this frame; the 0x0E hp_pct
            // takes over from there. while the `dead` routine is queued but its
            // fall-over has not started, hold idle instead of flashing cor? for the gap frame;
            // once ded? owns the pose via the completion motion, dead may be true again.
            let dead = ((is_self && self_dead)
                || snap.map(|s| s.dead).unwrap_or(false)
                || dead_from_action.is_some())
                && !(dead_from_action.is_some() && dead_fall_pending.contains(&world_id));

            let rest_kind = if is_self {
                self_rest_kind
            } else {
                snap.map(|s| s.rest).unwrap_or(RestKind::None)
            };

            let (forward_vel, strafe_vel) = if drives_from_self_input {
                if self_reactor_driven {
                    (0.0, 0.0)
                } else {
                    (self_move_forward, self_move_strafe)
                }
            } else if engaged {
                (sample.forward_component, sample.strafe_component)
            } else {
                (0.0, 0.0)
            };

            // Gait from the wire bytes for chase-owned entities (LSB UpdateSpeed multiplies
            // `speed` only, so run = speed > speed_base); self keeps its input-driven gait and
            // the transform-delta fallback covers everything else.
            let walking = if drives_from_self_input {
                self_walking
            } else if let Some((speed, speed_base)) = snap.and_then(|s| s.wire_gait) {
                actor_state::wire_walking(speed, speed_base)
            } else {
                infers_walk_gait(sample.speed)
            };

            let fishing_phase = if is_self {
                self_fishing_phase
            } else {
                snap.and_then(|s| s.fishing_phase)
            };

            // Special-pose wire state. Self never carries one (its root belongs to
            // apply_invis_flag_system); observed entities carry the state advanced in the
            // snapshot index above.
            let special = if is_self {
                ffxi_actor::actor_state::SpecialPose::default()
            } else {
                snap.map(|s| s.special).unwrap_or_default()
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

            let moving_flag = if drives_from_self_input && !self_reactor_driven {
                self_move_moving
            } else {
                motion.is_moving(motion_id)
            };
            // Retail's AnimationSpeed (SpeedBase * 0.1) scales walk/run clip playback relative to
            // the authored rate; only chase-owned moving entities get a non-unity scale, and
            // advance_actor_pose applies it to the locomotion tier alone.
            //
            // The stride scale matches a ground stride: the 0x45 Info movement byte (vekien/
            // xi-model-viewer ui/js/dat/inspect.js MOVEMENT_TYPE) says Flying and
            // Sliding mobs have no walk/run stride to match, so their locomotion clips play at
            // the authored rate. Walking/Large carry the wire scale; Unset (no CIB or 0xFF)
            // and an out-of-table byte keep today's behavior.
            let playback_rate = if moving_flag {
                snap.and_then(|s| s.wire_gait)
                    .map_or(1.0, |(_, speed_base)| match actor.movement_type {
                        MovementType::Flying | MovementType::Sliding => 1.0,
                        MovementType::Walking
                        | MovementType::Large
                        | MovementType::Unset
                        | MovementType::Unknown(_) => {
                            kuluu_snapshot::speed::anim_rate_scale(speed_base)
                        }
                    })
            } else {
                1.0
            };

            actor.facing_dir = 0.0;
            actor.inputs = ActorAnimInputs {
                moving: moving_flag,
                walking,
                playback_rate,
                forward_vel,
                strafe_vel,
                heading_rate: sample.heading_rate,
                engage_state,
                dead,
                rest: rest_kind,
                fishing_phase,
                special,
                mount_or_chocobo: snap.is_some_and(|s| s.mount_or_chocobo),
                ..Default::default()
            };

            let look_target_id = if is_self {
                self_target_id
            } else {
                snap.map(|s| s.face_target)
                    .filter(|&t| t != 0)
                    .and_then(|targid| index.id_by_targid.get(&targid).copied())
            };
            let look = look_target_id
                .filter(|&tid| tid != world_id)
                .and_then(|tid| {
                    actor_world_by_id.get(&tid).copied().or_else(|| {
                        index
                            .by_id
                            .get(&tid)
                            .map(|s| crate::scene::ffxi_to_bevy(s.pos))
                    })
                })
                .map(|base| {
                    let world = base + Vec3::Y * TARGET_LOOK_HEIGHT;
                    (actor_global.to_matrix(), world)
                });

            if is_self && actor.action.map(|a| a.cast_pose).unwrap_or(false) && !self_casting {
                actor.action = None;
                actor.action_clips.clear();
            }

            let mount_attach = mount_attach_by_rider.get(&world_id).copied();

            advance_actor_pose(
                &mut actor,
                elapsed_frames,
                look,
                mount_attach,
                animation_locked.contains(&world_id),
            );

            // Special-pose visibility: status INVISIBLE hides the model root outright (retail
            // destroys the actor on that byte; we keep one hidden actor instead). Nothing else
            // writes here: a completed one-shot holds its end frame pinned in the coordinator,
            // and retail never hides on clip completion - a worm's buried dig pose is occluded
            // by terrain exactly as there. Every other root belongs to
            // scene::apply_invis_flag_system, which resets it each frame.
            if special.hidden {
                *vis = Visibility::Hidden;
            }
        });

    for (actor, _, _, _) in &q_actors {
        registry
            .skin_mut(actor.skin_slot)
            .joints
            .set_from(&actor.world_pose);
    }

    if let Some(self_id) = self_id {
        if let Some((actor, _, _, _)) = q_actors.iter().find(|(a, _, _, _)| a.world_id == self_id) {
            rest.observe_exit_clip(matches!(actor.rest_phase, RestPlayback::Stopping { .. }));
        }
    }
}

const TARGET_LOOK_HEIGHT: f32 = 1.4;

const CAST_TIMEOUT_FRAMES: f32 = 60.0 * FRAME_RATE;

// The cast-motion clip keys on the retail spell DAT's magicType (research/xim
// DatResource.kt::castSuffix), which splits enfeebling across white/black — unlike
// the LSB magic skill. Fall back to the skill-derived suffix when the DAT is absent.
#[derive(Default, Resource)]
pub struct SpellSuffixCache {
    loaded: bool,
    table: Option<ffxi_dat::spell_info::SpellTable>,
}

impl SpellSuffixCache {
    pub(crate) fn suffix(&mut self, spell_id: u32) -> Option<&'static str> {
        if !self.loaded {
            self.loaded = true;
            if let Ok(root) = DatRoot::from_env_or_default() {
                self.table = Some(ffxi_dat::spell_info::SpellTable::open(root.root()));
            }
        }
        self.table
            .as_ref()
            .and_then(|t| t.lookup(spell_id as u16))
            .and_then(|s| s.magic_type.cast_suffix())
            .or_else(|| ffxi_vocab::magic::cast_suffix(spell_id))
    }
}

pub fn dispatch_action_overlay(
    events: Res<crate::snapshot::EventLog>,
    mut q_actors: Query<&mut FfxiRenderActor>,
    mut last_seen: Local<u64>,
    mut spell_suffix: ResMut<SpellSuffixCache>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
            animation,
            ..
        } = *ev
        else {
            continue;
        };
        let Some(mut actor) = q_actors.iter_mut().find(|a| a.world_id == actor_id) else {
            continue;
        };

        // An interrupt arrives on any start category carrying a "sp*" FourCC
        // (vendor/server/src/map/action/interrupts.cpp MagicInterrupt); treating it as a start would
        // re-arm the looping pose for CAST_TIMEOUT_FRAMES instead of dropping it.
        let start = matches!(action_kind, 7 | 9 | 10 | 12 | MAGIC_START_CATEGORY)
            .then(|| ffxi_vocab::magic::magic_start_routine(action_id))
            .flatten();
        if start.is_some_and(|m| m.interrupt) {
            // Only a pose that is still held (looping) needs dropping; one-shot starts have
            // already finished by the time an interrupt could matter.
            if actor.action.map(|a| a.looping).unwrap_or(false) {
                actor.action = None;
            }
            continue;
        }
        let cast_routine_id = start.map(|m| DatId::from_name(&m.id));
        let cast_suffix = match (action_kind == MAGIC_START_CATEGORY, cast_routine_id) {
            (true, None) => spell_suffix.suffix(action_id),
            _ => None,
        };
        // A FourCC start keeps its category's looping semantics: the generic `cast`/`calg`
        // holds loop until resolution, while `cate`/`cait` play once.
        let fourcc_looping = matches!(action_kind, 10 | 12);
        match cast_routine_id
            .map(|id| (id, action_kind == MAGIC_START_CATEGORY || fourcc_looping))
            .or_else(|| action_routine(action_kind, action_id, cast_suffix, animation))
        {
            None => {
                if actor.action.map(|a| a.looping).unwrap_or(false) {
                    actor.action = None;
                }
            }
            Some((mut routine, mut looping)) => {
                // a limb this model does not carry (no bti0/cti0/dti0 Motion
                // clip in its DAT) falls back to ati0 before the silent skip below.
                if action_kind == ffxi_proto::melee::CATEGORY_BASIC_ATTACK
                    && routine_motion_clip(&actor.routines, routine).is_none()
                {
                    (routine, looping) = (DatId::from_str("ati0"), false);
                }
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
                    cast_pose: action_kind == MAGIC_START_CATEGORY,
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
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    const AMBIENT_REF_LUX: f32 = 1000.0;
    const DIR_REF_LUX: f32 = 12000.0;

    const COLOR_BIAS: Vec3 = Vec3::new(1.4, 1.36, 1.45);
    const AMBIENT_BIAS_BELOW: f32 = 0.5;
    const AMBIENT_FLOOR: f32 = 0.12;
    // The 0x2F entity sun/moon diffuse is authored overbright (up to ~1.27 at noon);
    // clamping the model directional to 1.0 cropped that punch and flattened the form.
    const MODEL_DIR_MAX: f32 = 1.5;

    // research/xim EnvironmentSection.kt getModelLightingParams,168: actors are lit by the model block's
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
    // research/xim EnvironmentSection.kt getLightingParams lights: actors take a single time-blended
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

        point_pos: [Vec4::ZERO; crate::skinned_ffxi_material::MAX_POINT_LIGHTS],
        point_color: [Vec4::ZERO; crate::skinned_ffxi_material::MAX_POINT_LIGHTS],
        point_atten: [Vec4::ZERO; crate::skinned_ffxi_material::MAX_POINT_LIGHTS],
        time_params: Vec4::ZERO,
    };

    for actor in &q_actors {
        registry.skin_mut(actor.skin_slot).lighting = lighting.clone();
        for &slot in &actor.instance_slots {
            let inst = registry.instance_mut(slot);
            inst.flags.y = realistic;
            inst.flags.z = receive;
        }
    }
}

// Re-picking nearest-N scans and sorts every scene light per actor; nearest-N
// membership cannot visibly shift under sub-quarter-metre movement (light
// ranges are metres), so re-selection is gated on this displacement.
const POINT_LIGHT_RESELECT_EPSILON: f32 = 0.25;

// The nearest-N *selection* for an actor, cached across frames; the packed
// arrays are still refreshed from the live lights every frame so per-light
// flicker/night modulation keeps animating. `positions` pins the selection to
// the light set it was computed against: any positional drift or reorder of a
// selected slot (zone reload, /lights emitters) forces a re-pick.
struct ActorPointLightSelection {
    eval_pos: Vec3,
    authored: bool,
    count: usize,
    lights_len: usize,
    indices: Vec<u32>,
    positions: Vec<Vec3>,
}

impl ActorPointLightSelection {
    fn valid_for(
        &self,
        pos: Vec3,
        authored: bool,
        count: usize,
        lights: &[crate::zone_point_lights::ZonePointLight],
    ) -> bool {
        self.authored == authored
            && self.count == count
            && self.lights_len == lights.len()
            && self.eval_pos.distance_squared(pos)
                <= POINT_LIGHT_RESELECT_EPSILON * POINT_LIGHT_RESELECT_EPSILON
            && self
                .indices
                .iter()
                .zip(&self.positions)
                .all(|(&i, &p)| lights.get(i as usize).map(|l| l.world_pos) == Some(p))
    }
}

pub fn update_ffxi_actor_point_lights(
    active: Res<crate::zone_point_lights::ActiveSceneLights>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    chunk_lights: Res<crate::dat_mzb::ZoneChunkLightMap>,
    mut q_actors: Query<(&mut FfxiRenderActor, &GlobalTransform)>,
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    if active.lights.is_empty() {
        return;
    }
    let count = settings.model_light_count as usize;
    // The zone's own bindings are the point lights retail leaves in D3D slots
    // 2-5 while it draws a model over that chunk (ZoneRenderer.cpp ZoneRenderer::UpdateBlockLightSettings, :339-353;
    // ModelPartInstance.cpp ModelPartInstance::Draw only rebinds slots 0-1), so they light the
    // actor.
    let authored = chunk_lights.is_authored();

    for (mut actor, gt) in &mut q_actors {
        let pos = gt.translation();
        let cached_valid = actor
            .point_light_selection
            .as_ref()
            .is_some_and(|sel| sel.valid_for(pos, authored, count, &active.lights))
            && !chunk_lights.is_changed();
        if !cached_valid {
            let indices = match chunk_lights.lights_at(pos).filter(|_| authored) {
                Some(slots) => {
                    crate::zone_point_lights::authored_point_light_indices(&active.lights, &slots)
                }
                // A chunk that binds no light leaves its slots disabled; only a
                // zone with no binding table at all falls back to a distance pick.
                None if authored => Vec::new(),
                None => crate::zone_point_lights::nearest_point_light_indices(
                    pos,
                    &active.lights,
                    count,
                ),
            };
            let positions = indices
                .iter()
                .map(|&i| active.lights[i as usize].world_pos)
                .collect();
            actor.point_light_selection = Some(ActorPointLightSelection {
                eval_pos: pos,
                authored,
                count,
                lights_len: active.lights.len(),
                indices,
                positions,
            });
        }
        let Some(sel) = actor.point_light_selection.as_ref() else {
            continue;
        };
        let (point_pos, point_color, point_atten) =
            crate::zone_point_lights::point_light_arrays_for(&active.lights, &sel.indices);

        let lighting = &mut registry.skin_mut(actor.skin_slot).lighting;
        lighting.point_pos = point_pos;
        lighting.point_color = point_color;
        lighting.point_atten = point_atten;
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
mod mesh_dedup_tests {
    use super::*;

    fn synth_prepared(n_parts: usize) -> Arc<PreparedActor> {
        let skeleton = Skeleton {
            id: DatId::from_str("0000"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
        };
        let loaded = LoadedActor {
            skeleton: Arc::new(skeleton),
            skel_meshes: Vec::new(),
            effect_meshes: Vec::new(),
            textures: Vec::new(),
            animations: Arc::new(Vec::new()),
            battle_clips: Arc::new(Vec::new()),
            routines: Arc::new(HashMap::new()),
            action_assets: Arc::new(crate::scheduler_runtime::ActionAssets::default()),
            rejected_clips: Vec::new(),
            model_dat: "test.DAT".to_string(),
            cib: None,
        };
        let skel_built = (0..n_parts)
            .map(|_| BuiltGroup {
                mesh: Mesh::new(
                    PrimitiveTopology::TriangleList,
                    RenderAssetUsages::default(),
                ),
                texture_name: String::new(),
                tint: Vec4::ONE,
                joint_aabbs: Vec::new().into(),
            })
            .collect();
        Arc::new(PreparedActor {
            loaded,
            parts: PreparedParts {
                images: Vec::new(),
                skel_built,
                d3m_built: Vec::new(),
                bind_joints: FfxiJointMatrices::default(),
                bounds: None,
            },
            scale: 1.0,
        })
    }

    fn npc_key(file_id: u32) -> ActorPrepKey {
        ActorPrepKey::Npc {
            file_id,
            mipmaps: false,
            anisotropy: 1,
        }
    }

    #[test]
    fn cached_look_reuses_the_same_mesh_handles() {
        let mut meshes = Assets::<Mesh>::default();
        let mut cache = ActorPrepCache::default();
        let key = npc_key(1);
        cache.insert(key.clone(), synth_prepared(2));

        let first = cache.mesh_handles(&key, &mut meshes).expect("cached entry");
        let second = cache.mesh_handles(&key, &mut meshes).expect("cached entry");
        assert_eq!(first, second, "same look must reuse the same Mesh assets");
        assert_eq!(first.len(), 2);
        assert_eq!(
            meshes.iter().count(),
            2,
            "a re-spawn must not add new Mesh assets"
        );

        assert!(
            cache.mesh_handles(&npc_key(2), &mut meshes).is_none(),
            "an uncached look builds fresh handles at the call site"
        );
    }
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
        // The live path's layered resolution: the requested id first, then the idle family.
        let resolved = pose_clip_matches(&animations, overlay.iter(), selected_id);
        let clips = if resolved.is_empty() {
            pose_clip_matches(&animations, overlay.iter(), DatId::from_str("idl?"))
        } else {
            resolved
        };
        let mut ids: Vec<String> = clips.iter().map(|a| a.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn load_hume_m() -> Option<LoadedActor> {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return None;
        }

        Some(load_pc(1, false, &[], None, None, None).expect("load Hume M"))
    }

    #[test]
    fn live_idle_keeps_animating_with_nonselector_flags() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        bevy::tasks::ComputeTaskPool::get_or_init(Default::default);
        let mut results = Vec::new();
        // vendor/server/sql/mob_pools.sql: Damselfly sub=8, sheep sub=16, hare sub=0.
        for (name, file_id, animationsub) in [
            ("Damselfly", 1748, 8),
            ("Sheep", 1640, 16),
            ("Hare", 1568, 0),
            ("Damselfly without a dedicated sp1 clip", 1748, 1),
        ] {
            let loaded = load_npc(file_id).expect("installed retail NPC DAT");
            if animationsub == 1 {
                // sub=1 names the ini1 routine; on burrowing models its dig motion clip is sp1?,
                // and this model ships no such clip, so the override falls through to idle.
                let dig = DatId::from_str("sp1?");
                assert!(!loaded
                    .all_animations()
                    .iter()
                    .any(|clip| clip.id.parameterized_match(&dig)));
            }
            let idle_duration = loaded
                .all_animations()
                .iter()
                .filter(|clip| clip.id.parameterized_match(&DatId::from_str("idl?")))
                .map(SkeletonAnimation::length_in_frames)
                .fold(0.0f32, f32::max);
            assert!(idle_duration > 0.0, "{name} must have idle clips");
            let mut app = App::new();
            app.init_resource::<Time>()
                .init_resource::<crate::snapshot::SceneState>()
                .init_resource::<combat_stance::EntityMotion>()
                .init_resource::<combat_stance::RestStance>()
                .init_resource::<combat_stance::WalkMode>()
                .init_resource::<combat_stance::SelfMoveIntent>()
                .init_resource::<FfxiSkinRegistry>()
                .init_resource::<crate::scene::Target>()
                .init_resource::<crate::scene::TrackedEntities>()
                .add_systems(Update, tick_live_ffxi_actors);
            let skin = app
                .world_mut()
                .resource_mut::<FfxiSkinRegistry>()
                .alloc_skin();
            let actor_entity = app
                .world_mut()
                .spawn((
                    make_render_actor(&loaded, skin, Vec::new(), 1, 0.0, 1.0),
                    GlobalTransform::default(),
                    Visibility::Inherited,
                ))
                .id();
            let snapshot = &mut app
                .world_mut()
                .resource_mut::<crate::snapshot::SceneState>()
                .snapshot;
            snapshot.zone_id = Some(103);
            snapshot.entities.push(kuluu_snapshot::Entity {
                id: 1,
                act_index: 1,
                kind: kuluu_snapshot::EntityKind::Mob,
                name: Some(name.into()),
                pos: kuluu_snapshot::Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                animation: 0,
                animationsub: animationsub | 4,
                mount: None,
                status: 1,
                char_flags: Default::default(),
                monstrosity: false,
                name_vis: None,
            });
            let tick = |app: &mut App| {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / FRAME_RATE));
                app.update();
            };
            tick(&mut app);
            app.world_mut()
                .resource_mut::<crate::snapshot::SceneState>()
                .snapshot
                .entities[0]
                .animationsub = animationsub;
            for _ in 0..(idle_duration.ceil() as usize * 2 + 1) {
                tick(&mut app);
            }
            let pose = app
                .world()
                .get::<FfxiRenderActor>(actor_entity)
                .unwrap()
                .world_pose()
                .to_vec();
            let mut moved = false;
            for _ in 0..(idle_duration.ceil() as usize + 1) {
                tick(&mut app);
                let actor = app.world().get::<FfxiRenderActor>(actor_entity).unwrap();
                moved |= pose
                    .iter()
                    .zip(actor.world_pose())
                    .any(|(a, b)| !a.abs_diff_eq(*b, 1e-5));
            }
            let actor = app.world().get::<FfxiRenderActor>(actor_entity).unwrap();
            // sub=1 keeps the ini1 override active (the model ships no clip for it); every other
            // sub here settles to plain locomotion.
            let healthy =
                moved && (animationsub == 1 || actor.inputs.special.active_routine.is_none());
            results.push((
                healthy,
                format!(
                    "{name}: moved={moved}, special={:?}, selected={:?}, resolved={:?}, frame={}",
                    actor.inputs.special, actor.current_clip, actor.last_clip, actor.last_frame
                ),
            ));
        }
        assert!(
            results.iter().all(|(moved, _)| *moved),
            "{}",
            results
                .iter()
                .map(|(_, detail)| detail.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn worm_dig_keeps_its_dedicated_one_shot() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        // Installed ROM/5/64.DAT, the tunnel worm reference.
        let loaded =
            load_npc(crate::look_resolver::npc_dat_id(0x01a8)).expect("installed worm DAT");
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        // sub=1 names the ini1 routine; its first Motion stage is the dig clip (the clip
        // comes from the routine record, not a hard-coded mapping).
        let dig = routine_motion_clip(&actor.routines, DatId::from_name(b"ini1"))
            .expect("worm ini1 routine carries a motion stage");
        assert!(
            dig.parameterized_match(&DatId::from_str("sp1?")),
            "worm dig clip is sp1?"
        );
        let duration = actor
            .animations
            .iter()
            .filter(|clip| clip.id.parameterized_match(&dig))
            .map(SkeletonAnimation::length_in_frames)
            .fold(0.0f32, f32::max);
        assert!(duration > 0.0, "worm has dedicated dig clips");
        actor.inputs.special.active_routine = Some(*b"ini1");
        for _ in 0..(duration.ceil() as usize * 2 + 1) {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        assert!(actor
            .last_clip
            .is_some_and(|id| id.parameterized_match(&dig)));
        let buried_pose = actor.world_pose().to_vec();
        // The one-shot holds its end frame: no further advance moves the pose.
        advance_actor_pose_standalone(&mut actor, duration, None);
        assert_eq!(actor.world_pose(), buried_pose);
    }

    #[test]
    fn nameplate_locators_match_installed_retail_dat_measurements() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        // Installed DATs measured alongside FFXiMain.dll 0x1002b9a0 (kuluu-81r8).
        let models = [
            (
                "Tarutaru",
                load_pc(5, false, &[], None, None, None).unwrap(),
                1.3,
            ),
            (
                "Galka",
                load_pc(8, false, &[], None, None, None).unwrap(),
                2.6,
            ),
            ("Damselfly", load_npc(1748).unwrap(), 3.5),
        ];
        for (name, loaded, expected_y) in models {
            let locator = crate::scene::NameplateLocator::from_skeleton(&loaded.skeleton, 1.0);
            let expected = Vec3::Y * expected_y;
            assert!(
                (locator.offset.unwrap() - expected).length() < 1e-5,
                "{name}"
            );
            let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
            for frame in [0.0, 8.0, 16.0] {
                advance_actor_pose_standalone(&mut actor, frame, None);
                let current =
                    crate::scene::NameplateLocator::from_skeleton(&actor.skeleton, actor.scale);
                assert_eq!(current.offset, locator.offset, "{name} at {frame}");
            }
        }
    }

    #[test]
    fn bind_pose_bounds_are_feet_origin_and_race_specific() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let mut heights: std::collections::HashMap<u8, f32> = std::collections::HashMap::new();
        for race in 1..=8u8 {
            let Ok(actor) = load_pc(race, false, &[], None, None, None) else {
                continue;
            };
            let Some((lo, hi)) = actor.bind_pose_bounds(0.0, 1.0) else {
                panic!("race {race}: no bind-pose bounds");
            };
            assert!(
                lo.y.abs() < 0.05,
                "race {race} feet not at the origin: min.y={:.3}",
                lo.y
            );
            let h = hi.y - lo.y;
            assert!(
                h.is_finite() && h > 0.5,
                "race {race}: implausible height {h}"
            );
            heights.insert(race, h);
        }
        // Hume M (1) vs Elvaan M (3): the bead's own anchor — Elvaan bakes to
        // ~2.08 (vendor/server CharRace order, charentity.h).
        let hume = *heights.get(&1).expect("Hume M loaded");
        let elvaan = *heights.get(&3).expect("Elvaan M loaded");
        assert!(
            (1.5..2.1).contains(&hume),
            "Hume M height {hume} drifted out of band"
        );
        assert!(
            (1.9..2.4).contains(&elvaan),
            "Elvaan M height {elvaan} drifted out of the ~2.08 band"
        );
        // The whole point: races must not all anchor at one height.
        let hs = heights.values().copied();
        let (lo_h, hi_h) = (
            hs.clone().fold(f32::INFINITY, f32::min),
            hs.fold(f32::NEG_INFINITY, f32::max),
        );
        assert!(
            hi_h - lo_h > 0.5,
            "races collapsed to one height: {heights:?}"
        );
    }

    /// Skinned Y extent of one posed actor, the same skinning as
    /// `bind_pose_bounds`.
    fn skinned_y_range(loaded: &LoadedActor, pose: &[Mat4]) -> (f32, f32) {
        let basis = ffxi_to_bevy_basis();
        let joint_count = loaded.skeleton.joints.len();
        let occlusion: std::collections::HashSet<u8> =
            loaded.skel_meshes.iter().map(|m| m.occlude_type).collect();
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for skel_mesh in &loaded.skel_meshes {
            for buffer in &skel_mesh.meshes {
                if is_occluded(buffer, &occlusion) {
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
                    lo = lo.min(wp.y);
                    hi = hi.max(wp.y);
                }
            }
        }
        (lo, hi)
    }

    #[test]
    fn npc_bind_pose_does_not_describe_the_drawn_extent() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let loaded = load_npc(1556).expect("load Huge Hornet dat 1556");
        let Some((bind_lo, bind_hi)) = loaded.bind_pose_bounds(0.0, 1.0) else {
            panic!("Huge Hornet: no bind-pose bounds");
        };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        // The idle hover bobs between two heights; the lowest drawn point
        // across sampled frames is what a static anchor must clear.
        let mut drawn_lo = f32::INFINITY;
        for frame in [0.0f32, 8.0, 16.0] {
            advance_actor_pose_standalone(&mut actor, frame, None);
            let (lo, _) = skinned_y_range(&loaded, actor.world_pose());
            drawn_lo = drawn_lo.min(lo);
        }
        assert!(
            drawn_lo > bind_hi.y + 0.5,
            "Huge Hornet idle no longer lifts off its rest pose: drawn lo {drawn_lo:.3} vs bind [{bind_lo:.3}, {bind_hi:.3}]"
        );
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

    /// Pins the whole ridden-chocobo model path against the real DAT: the
    /// FFXiMain race table entry, its body parts, and the `chi?` seat clip the
    /// rider needs. Self-skips without a retail install.
    #[test]
    fn chocobo_mount_race_loads_with_a_seat_clip() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let race = chocobo_race_for_colour(kuluu_snapshot::ChocoboColour::Yellow);
        let actor = load_mount_race(race).expect("yellow chocobo race config");
        assert!(
            !actor.skel_meshes.is_empty(),
            "the race config ships no meshes of its own; the body comes from the \
             equipment table and an empty result means that lookup broke"
        );
        assert!(
            actor
                .animations
                .iter()
                .any(|a| a.id.as_str().starts_with("chi")),
            "the mount's carrying pose is chi?; got {:?}",
            actor
                .animations
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>()
        );
        // Every colour is its own race config, so they must not collide.
        let black = chocobo_race_for_colour(kuluu_snapshot::ChocoboColour::Black);
        assert_ne!(race, black);
        assert!(load_mount_race(black).is_ok(), "black chocobo race config");
    }

    /// Why `mount_seat_local` reads a chocobo's seat off its spine instead of
    /// the saddle joint every other mount declares: a chocobo race skeleton
    /// leaves that whole per-race block pointing at joint 0 with a zero offset,
    /// so the lookup resolves to the ground and drops the rider through the
    /// floor. Self-skips without a retail install.
    #[test]
    fn chocobo_race_skeletons_define_no_saddle_joints() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let race = chocobo_race_for_colour(kuluu_snapshot::ChocoboColour::Yellow);
        let actor = load_mount_race(race).expect("yellow chocobo race config");
        let pose = pose_world(
            &actor.skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        for rider_race in 1..=8u8 {
            let joint = saddle_joint_index(rider_race).expect("playable race");
            assert_eq!(
                standard_joint_world_position(&pose, &actor.skeleton, joint),
                Some(Vec3::ZERO),
                "std {joint} is unexpectedly a real saddle joint; reading the \
                 spine instead would then be discarding real data"
            );
        }
    }

    /// The seat has to move, which is the whole reason it is read from a pose
    /// rather than fixed: a chocobo's back rises and falls through its gallop,
    /// and a rider pinned to one height has the saddle saw up through them.
    /// Self-skips without a retail install.
    #[test]
    fn a_chocobos_seat_rises_and_falls_with_its_gait() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let mount = load_mount_race(chocobo_race_for_colour(
            kuluu_snapshot::ChocoboColour::Yellow,
        ))
        .expect("yellow chocobo race config");
        let run = DatId::from_str("run?");
        let clips: Vec<SkeletonAnimation> = mount
            .animations
            .iter()
            .filter(|c| c.id.parameterized_match(&run))
            .cloned()
            .collect();
        assert!(!clips.is_empty(), "the chocobo carries its own run clip");
        let length = clips
            .iter()
            .map(|c| c.length_in_frames())
            .fold(0.0_f32, f32::max);

        const SAMPLES: usize = 8;
        let seats: Vec<f32> = (0..SAMPLES)
            .map(|k| {
                let t = length * k as f32 / SAMPLES as f32;
                let clips = clips.clone();
                let pose = pose_world(
                    &mount.skeleton,
                    move |joint| {
                        clips
                            .iter()
                            .find_map(|c| c.get_joint_transform(joint as u32, t))
                    },
                    ffxi_actor::skeleton_instance::RootTransform::identity(),
                    &[],
                );
                chocobo_seat_local(&pose, CHOCOBO_SEAT_ABOVE_BACK)
                    .expect("a chocobo skeleton reaches the spine")
                    .y
            })
            .collect();

        let lo = seats.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = seats.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        // Rider bodies are around two yalms tall, so a seat that wanders this
        // far is plainly visible against one.
        assert!(
            hi - lo > 0.05,
            "the seat barely moves over a gallop ({lo}..{hi}); either the clip \
             stopped being found or the joint stopped being the spine"
        );
    }

    /// The rider's seat clips live in a DAT that is only loaded while mounted.
    #[test]
    fn mounted_rider_gains_the_seat_clip_an_unmounted_one_lacks() {
        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let has_chi = |mounted: bool| {
            load_pc(1, mounted, &[], None, None, None)
                .expect("load Hume M")
                .animations
                .iter()
                .any(|a| a.id.as_str().starts_with("chi"))
        };
        assert!(has_chi(true), "a rider must have chi? to sit on a chocobo");
        assert!(
            !has_chi(false),
            "the seat clips must not be paid for by every PC on foot"
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

    #[test]
    fn only_the_middle_rest_phase_loops() {
        let anims = vec![synth_anim(b"rx00", 4), synth_anim(b"rx20", 4)];
        let mut phase = RestPlayback::Inactive;
        let looping = |phase: &mut RestPlayback, desired| {
            advance_rest_phase(phase, desired, &anims, 1.0)
                .map(|_| matches!(phase, RestPlayback::Looping { .. }))
        };

        assert_eq!(
            looping(&mut phase, RestKind::Kneel),
            Some(false),
            "the kneel-down must play once, not loop back to standing"
        );
        for _ in 0..4 {
            looping(&mut phase, RestKind::Kneel);
        }
        assert_eq!(looping(&mut phase, RestKind::Kneel), Some(true));

        assert_eq!(
            looping(&mut phase, RestKind::None),
            Some(false),
            "the stand-up must play once, not replay the kneel from frame 0"
        );
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
                            random_group: None,
                            local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
                            model_transform: None,
                            follow_points: None,
                            screen_color: None,
                            actor_fade: None,
                            idle_transition_time: None,
                            flinch_duration: None,
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

    fn synth_death_routine(stages: &[(&[u8; 4], u16, u32)]) -> HashMap<DatId, Scheduler> {
        use ffxi_dat::scheduler::{SchedulerStage, TimedStage};
        let name = *b"dead";
        let mut out = HashMap::new();
        out.insert(
            DatId::from_name(&name),
            Scheduler {
                name,
                stages: stages
                    .iter()
                    .map(|&(clip, duration_frames, frame)| TimedStage {
                        frame,
                        stage: SchedulerStage {
                            kind: StageKind::Motion,
                            raw_type: 0x05,
                            delay_frames: 0,
                            duration_frames,
                            id: *clip,
                            max_loops: 1,
                            transition_in: 0,
                            transition_out: 0,
                            random_group: None,
                            local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
                            model_transform: None,
                            follow_points: None,
                            screen_color: None,
                            actor_fade: None,
                            idle_transition_time: None,
                            flinch_duration: None,
                        },
                    })
                    .collect(),
            },
        );
        out
    }

    #[test]
    fn death_collapse_clip_reads_the_dead_routine_stages() {
        let routines = synth_death_routine(&[(b"ded?", 116, 0), (b"cor?", 2, 116)]);
        let (clip, frames) = death_collapse_clip(&routines).expect("collapse stage");
        assert_eq!(clip.as_str(), "ded?");
        assert_eq!(frames, 58.0, "duration_frames is in half-frame units");
    }

    #[test]
    fn death_collapse_clip_needs_a_corpse_stage_to_settle_on() {
        let routines = synth_death_routine(&[(b"ded?", 116, 0)]);
        assert_eq!(death_collapse_clip(&routines), None);
        assert_eq!(death_collapse_clip(&HashMap::new()), None);
    }

    #[test]
    fn death_phase_selects_the_collapse_then_releases_to_the_corpse_pose() {
        let routines = synth_death_routine(&[(b"ded?", 116, 0), (b"cor?", 2, 116)]);
        let (collapse_id, collapse_frames) = death_collapse_clip(&routines).unwrap();

        let mut phase = actor_state::DeathPhase::Unobserved;
        let mut step = |dead: bool| {
            phase = actor_state::next_death_phase(phase, dead, collapse_frames, 1.0);
            match phase {
                actor_state::DeathPhase::Collapsing { .. } => Some(collapse_id.as_str()),
                _ => None,
            }
        };

        assert_eq!(step(false), None);
        for _ in 0..collapse_frames as u32 {
            assert_eq!(step(true).as_deref(), Some("ded?"));
        }
        for _ in 0..600 {
            assert_eq!(
                step(true),
                None,
                "the collapse must not replay under the held corpse pose"
            );
        }
    }

    // `dat-routine-stages <pc skeleton> dead`, half-frames halved, over races 1..=8
    // (skeletons 7072/10248/13424/16600/19776 shared by both Tarutaru/23176/26352):
    // the collapse length is authored per race, so it has to be read from the routine.
    const PC_COLLAPSE_FRAMES: [(u8, f32); 8] = [
        (1, 58.0),
        (2, 78.0),
        (3, 43.0),
        (4, 68.0),
        (5, 34.0),
        (6, 34.0),
        (7, 60.0),
        (8, 60.0),
    ];

    // Retail-DAT guard (skips without an install) for the oracle the phase machine is
    // built on: every PC `dead` routine is a one-shot `ded?` collapse followed by
    // `cor?`, and `cor?` is itself static -- so what retail holds forever is the corpse
    // frame, never the collapse.
    #[test]
    fn retail_dead_routine_is_a_one_shot_collapse_into_a_static_corpse_pose() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return;
        }

        for (race, expected_frames) in PC_COLLAPSE_FRAMES {
            let actor = load_pc(race, false, &[], None, None, None).expect("load PC race");
            let routines = actor.all_routines();

            let (collapse_id, collapse_frames) =
                death_collapse_clip(&routines).expect("PC dead routine");
            assert_eq!(collapse_id.as_str(), "ded?");
            assert_eq!(
                collapse_frames, expected_frames,
                "race {race} collapse length"
            );
            assert_eq!(
                routine_motion_clip_last(&routines, actor_state::death_routine_id())
                    .map(|d| d.as_str()),
                Some("cor?".to_string()),
                "race {race} dead routine settles on the corpse pose"
            );

            let animations = actor.all_animations();
            let clips = |id: DatId| -> Vec<&SkeletonAnimation> {
                animations
                    .iter()
                    .filter(|a| a.id.parameterized_match(&id))
                    .collect()
            };

            let collapse = clips(collapse_id);
            assert!(
                !collapse.is_empty(),
                "race {race} collapse clip is loadable"
            );
            for a in &collapse {
                assert!(
                    a.length_in_frames() > 1.0,
                    "race {race} {} is a motion, not a pose",
                    a.id.as_str()
                );
            }

            let corpse = clips(actor_state::corpse_pose_id());
            assert!(!corpse.is_empty(), "race {race} corpse pose is loadable");
            for a in &corpse {
                for set in a.key_frame_sets.values() {
                    let first = set[0];
                    let last = set[a.num_frames - 1];
                    assert!(
                        same_orientation(first.rotation, last.rotation),
                        "race {race} {} rotates between its first and last keyframe",
                        a.id.as_str()
                    );
                    assert!(
                        same_component(first.translation, last.translation),
                        "race {race} {} translates between its first and last keyframe",
                        a.id.as_str()
                    );
                    assert!(
                        same_component(first.scale, last.scale),
                        "race {race} {} scales between its first and last keyframe",
                        a.id.as_str()
                    );
                }
            }
        }
    }

    // Retail stores `cor?` as compressed keyframes, so its first and last frame
    // decode to the same pose only up to f32 round-off: measured over all seven PC
    // skeletons the worst gap is 2.4e-7 on a translation and 2.4e-7 on |q1.q2| - 1.
    const STATIC_POSE_TOLERANCE: f32 = 1e-6;

    fn same_component(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| (x - y).abs() <= STATIC_POSE_TOLERANCE)
    }

    // A quaternion and its negation name the same orientation, and retail's `cor?`
    // does store one bone's identity rotation as `[0,0,0,1]` on the first keyframe
    // and `[0,0,0,-1]` on the last, so the static-pose check compares orientations
    // rather than raw components.
    fn same_orientation(a: [f32; 4], b: [f32; 4]) -> bool {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        (dot.abs() - 1.0).abs() <= STATIC_POSE_TOLERANCE
    }

    // Retail-DAT end-to-end (skips without an install) for
    // .agents/skills/retail-observe/references/death-ko-behavior.md: "Death plays a
    // collapse motion once and holds the final corpse frame -- it is not a looping
    // idle."
    #[test]
    fn death_collapse_plays_once_then_holds_the_corpse_frame() {
        let Some(loaded) = load_hume_m() else {
            return;
        };
        let routines = loaded.all_routines();
        let (collapse_id, collapse_frames) =
            death_collapse_clip(&routines).expect("HumeM dead routine");
        let corpse_id = actor_state::corpse_pose_id();
        let is = |actor: &FfxiRenderActor, id: &DatId| {
            actor.last_clip.is_some_and(|c| c.parameterized_match(id))
        };

        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        advance_actor_pose_standalone(&mut actor, 1.0, None);
        actor.inputs.dead = true;

        let mut collapse_frames_seen = 0;
        for _ in 0..collapse_frames as usize {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
            collapse_frames_seen += usize::from(is(&actor, &collapse_id));
        }
        assert_eq!(
            collapse_frames_seen, collapse_frames as usize,
            "the collapse runs for the routine stage's whole duration"
        );

        for _ in 0..collapse_frames as usize {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        assert!(is(&actor, &corpse_id), "the collapse settles on `cor?`");

        let held = actor.world_pose().to_vec();
        for _ in 0..600 {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
            assert!(!is(&actor, &collapse_id), "the collapse must not replay");
        }
        assert_eq!(
            actor.world_pose(),
            held,
            "the corpse frame is held, not re-animated"
        );
    }

    // A homepoint warp is a zone change, and zoning in while still KO'd is the one
    // case where the client sees `dead` without having watched the death
    // (death-ko-behavior.md, "0x00A LOGIN carries a DeadCounter").
    #[test]
    fn a_death_the_client_never_watched_holds_the_corpse_frame() {
        let Some(loaded) = load_hume_m() else {
            return;
        };
        let (collapse_id, _) =
            death_collapse_clip(&loaded.all_routines()).expect("HumeM dead routine");

        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        advance_actor_pose_standalone(&mut actor, 1.0, None);
        reset_actor_pose_state(&mut actor, 1.0);
        actor.inputs.dead = true;

        for _ in 0..600 {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
            assert!(
                !actor
                    .last_clip
                    .is_some_and(|c| c.parameterized_match(&collapse_id)),
                "a death the client never watched must not replay the collapse"
            );
        }
        assert!(actor
            .last_clip
            .is_some_and(|c| c.parameterized_match(&actor_state::corpse_pose_id())));
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

    // Retail-DAT guard (skips without an install): the cast-start effects now run through the
    // scheduler with Motion stages suppressed (kuluu-ky8c), so the overlay must remain the sole
    // owner of the looping cast pose — HumeM's `cabk` still yields its mb0? clip here.
    #[test]
    fn cast_overlay_still_owns_the_looping_pose() {
        const HUME_M_SKELETON_FILE: u32 = 7072;

        let (routine, looping) =
            action_routine(MAGIC_START_CATEGORY, 0, Some("bk"), None).expect("black magic poses");
        assert_eq!(routine.as_str(), "cabk");
        assert!(looping, "the cast pose loops until the cast resolves");

        let Ok(root) = DatRoot::from_env_or_default() else {
            return;
        };
        let Ok(loc) = root.resolve(HUME_M_SKELETON_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let (schedulers, _) = crate::scheduler_runtime::parse_action_bytes(&bytes);
        let routines: HashMap<DatId, Scheduler> = schedulers
            .into_iter()
            .map(|s| (DatId::from_name(&s.name), s))
            .collect();
        assert_eq!(
            routine_motion_clip(&routines, routine).map(|d| d.as_str()),
            Some("mb0?".to_string()),
            "the cast pose clip is still resolved from the caster's own routine"
        );
    }

    // Retail-DAT guard (skips without an install). The melee hit chain resolves `ef h` (the hit
    // spark) and `se h`/`skaz` (the impact/whoosh) out of the EQUIPPED WEAPON's DAT, and `chit`
    // out of the skeleton — research/xim EffectRoutineInstance.kt searchAssociatedDir
    // walks every one of the actor's animation directories. load_pc drops an equipment file whose
    // `collect_skel_meshes()` is empty, so a weapon that stops contributing meshes would silently
    // take the whole spark chain with it.
    #[test]
    fn equipped_weapon_routines_reach_the_actor_lookup() {
        // look_resolver::PC_MODEL_IDS[HumeM][main-hand] base — main-hand weapon model 0.
        const HUME_M_MAIN_WEAPON_FILE: u32 = 8392;

        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let mut equipment = vec![HUME_M_MAIN_WEAPON_FILE];
        equipment.extend(
            (1u16..=5)
                .filter_map(|slot| crate::look_resolver::resolve_equipment_slot(slot << 12, 1)),
        );
        let actor = load_pc(
            1,
            false,
            &equipment,
            None,
            Some(HUME_M_MAIN_WEAPON_FILE),
            None,
        )
        .expect("load Hume M with a main-hand weapon");
        for id in ["ef h", "se h", "skaz", "chit"] {
            assert!(
                actor.routines.contains_key(&DatId::from_str(id)),
                "actor routine lookup is missing `{id}`"
            );
        }
    }

    #[test]
    fn action_routing_maps_categories() {
        let r = |k, cmd_arg, suffix, animation| {
            action_routine(k, cmd_arg, suffix, animation).map(|(d, looping)| (d.as_str(), looping))
        };

        assert_eq!(r(1, 0, None, None), Some(("ati0".to_string(), false)));

        // BATTLE2's animation field picks the limb routine; absent or
        // out-of-range values (and Throw, which has no limb routine) fall back to ati0.
        assert_eq!(r(1, 0, None, Some(0)), Some(("ati0".to_string(), false)));
        assert_eq!(r(1, 0, None, Some(1)), Some(("bti0".to_string(), false)));
        assert_eq!(r(1, 0, None, Some(2)), Some(("cti0".to_string(), false)));
        assert_eq!(r(1, 0, None, Some(3)), Some(("dti0".to_string(), false)));
        assert_eq!(
            r(1, 0, None, Some(4)),
            Some(("ati0".to_string(), false)),
            "Throw has no limb routine"
        );
        assert_eq!(
            r(1, 0, None, Some(9)),
            Some(("ati0".to_string(), false)),
            "out-of-range falls back"
        );

        assert_eq!(r(8, 0, Some("wh"), None), Some(("cawh".to_string(), true)));

        assert_eq!(r(8, 0, Some("bk"), None), Some(("cabk".to_string(), true)));

        assert_eq!(r(8, 0, None, None), Some(("cast".to_string(), true)));

        // the start categories play the packet's FourCC; a payload that is not one
        // falls back to the category's hard-coded retail default.
        const CATE: u32 = 0x65746163;
        const CAIT: u32 = 0x74696163;
        const CALG: u32 = 0x676C6163;

        assert_eq!(r(7, CATE, None, None), Some(("cate".to_string(), false)));
        assert_eq!(
            r(7, 0, None, None),
            Some(("cate".to_string(), false)),
            "fallback"
        );
        assert_eq!(r(9, CAIT, None, None), Some(("cait".to_string(), false)));
        assert_eq!(
            r(9, 0, None, None),
            Some(("cait".to_string(), false)),
            "fallback"
        );
        assert_eq!(r(12, CALG, None, None), Some(("calg".to_string(), true)));
        assert_eq!(
            r(12, 0, None, None),
            Some(("calg".to_string(), true)),
            "fallback"
        );
        assert_eq!(
            r(10, 0, None, None),
            Some(("cast".to_string(), true)),
            "fallback"
        );

        for finish in [2u8, 3, 4, 5, 6, 0] {
            assert_eq!(
                r(finish, 0, None, None),
                None,
                "category {finish} should not pose"
            );
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
        let ids: Vec<String> = pose_clip_matches(&anims, battle.iter(), swing)
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert!(
            ids.contains(&"at00".to_string()) && ids.contains(&"at01".to_string()),
            "swing resolves to at00+at01 (got {ids:?})"
        );
    }

    /// B: feed 10 consecutive POS updates (~400 ms apart) with the same gait; the walk clip must
    /// register exactly once and its frame cursor must advance monotonically (mod length), never
    /// resetting to 0 mid-run. A re-registration on an unchanged gait would snap the cursor back
    /// toward frame 0, which this catches step by step.
    #[test]
    fn locomotion_clip_loops_continuously_across_updates() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        let inputs = inputs_for_pose(PoseState::Walk, false);

        const UPDATES: usize = 10;
        const FRAME_STEP: f32 = 24.0; // ~400 ms at 60 fps: one server tick's worth of frames

        // First update registers the walk clip set. Capture current_clip and every registered
        // non-idle clip's cursor; an unchanged gait must never re-select or re-register them.
        actor.inputs = inputs;
        advance_actor_pose_standalone(&mut actor, FRAME_STEP, None);
        let first_clip = actor
            .current_clip
            .expect("a locomotion clip is selected on the first update");
        assert!(
            !first_clip.0.as_str().starts_with("idl"),
            "expected a walk clip, got the idle fallback {}",
            first_clip.0.as_str()
        );

        #[derive(Clone)]
        struct Reg {
            id: DatId,
            frame: f32,
        }
        let mut regs: Vec<Reg> = actor
            .coordinator
            .animations
            .iter()
            .flatten()
            .filter_map(|slot| slot.current_animation.as_ref())
            .filter(|c| !c.animation.id.as_str().starts_with("idl"))
            .map(|c| Reg {
                id: c.animation.id,
                frame: c.current_frame,
            })
            .collect();
        assert!(
            !regs.is_empty(),
            "no locomotion clip registered after the first update"
        );

        for i in 2..=UPDATES {
            actor.inputs = inputs;
            advance_actor_pose_standalone(&mut actor, FRAME_STEP, None);

            // current_clip is written only inside the registration gate; if it never changes across
            // updates, register_animation was never called again -> registered exactly once.
            assert_eq!(
                actor.current_clip,
                Some(first_clip),
                "update {i}: unchanged gait re-selected the clip"
            );

            for r in regs.iter_mut() {
                let (frame_now, len_now) = actor
                    .coordinator
                    .animations
                    .iter()
                    .flatten()
                    .filter_map(|slot| slot.current_animation.as_ref())
                    .find(|c| c.animation.id == r.id)
                    .map(|c| (c.current_frame, c.animation.length_in_frames()))
                    .unwrap_or_else(|| {
                        panic!(
                            "update {i}: clip {} vanished from the coordinator",
                            r.id.as_str()
                        )
                    });
                // The cursor must advance forward by exactly one frame step (mod length); a reset to 0
                // on an unchanged gait would break this. Both representatives of the residue are
                // accepted: SkeletonAnimationContext::apply_loop_bounds wraps with `> length`, so a
                // cursor landing exactly on length holds there for one step instead of reading 0 (the
                // two sample identically at loop closure).
                let expected = (r.frame + FRAME_STEP).rem_euclid(len_now);
                assert!(
                    (frame_now - expected).abs() < 1e-3 || (frame_now - (expected + len_now)).abs() < 1e-3,
                    "update {i}: clip {} cursor reset mid-run: {} -> {} (expected ~{expected} mod {len_now})",
                    r.id.as_str(),
                    r.frame,
                    frame_now
                );
                r.frame = frame_now;
            }
        }
    }
}

#[cfg(test)]
mod actor_bounds_tests {
    use super::*;
    use ffxi_dat::skel_mesh::{RenderProperties, SkinVertex};

    // ffxi_zone_material::AMBIENT_FLOOR is tuned once for terrain and actors together, which
    // only works while both decode their vertex colour at the same scale.
    #[test]
    fn actor_and_zone_vertex_colour_decode_at_the_same_scale() {
        assert_eq!(
            ACTOR_VERTEX_COLOR_DIVISOR,
            ffxi_dat::mmb::VERTEX_COLOR_DIVISOR
        );
    }

    fn synth_vertex(q0: Vec3, q1: Vec3, w: f32, j0: u16, j1: u16) -> SkinVertex {
        SkinVertex {
            p0: (q0 * w).to_array(),
            p1: (q1 * (1.0 - w)).to_array(),
            n0: [0.0; 3],
            n1: [0.0; 3],
            u: 0.0,
            v: 0.0,
            joint0_weight: w,
            joint1_weight: 1.0 - w,
            joint_index0: j0,
            joint_index1: j1,
            color: [255; 4],
        }
    }

    fn synth_buffer(samples: &[(Vec3, Vec3, f32, u16, u16)]) -> MeshBuffer {
        MeshBuffer {
            mesh_type: MeshType::Mesh,
            texture_name: String::new(),
            render_properties: RenderProperties::default(),
            vertices: samples
                .iter()
                .map(|&(q0, q1, w, j0, j1)| synth_vertex(q0, q1, w, j0, j1))
                .collect(),
        }
    }

    const SAMPLES: [(Vec3, Vec3, f32, u16, u16); 4] = [
        (
            Vec3::new(0.1, 0.5, -0.2),
            Vec3::new(0.3, -0.1, 0.4),
            0.75,
            0,
            1,
        ),
        (
            Vec3::new(-0.4, 0.2, 0.6),
            Vec3::new(0.0, 0.9, -0.3),
            0.5,
            1,
            2,
        ),
        (Vec3::new(0.2, -0.6, 0.1), Vec3::ZERO, 1.0, 2, 0),
        (
            Vec3::new(0.05, 0.0, 0.35),
            Vec3::new(-0.25, 0.15, 0.0),
            0.25,
            0,
            2,
        ),
    ];

    #[test]
    fn built_meshes_have_no_builtin_position_attribute() {
        let mesh = build_mesh(&synth_buffer(&SAMPLES), 3);
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_none(),
            "actor meshes must not gain ATTRIBUTE_POSITION: calculate_bounds would \
             start writing bind-space Aabbs and silently flip the culling semantics \
             the manual joint-bounds path owns"
        );
        assert!(mesh.attribute(ATTR_POSITION0).is_some());
    }

    #[test]
    fn skinned_positions_lie_inside_joint_bound_union() {
        let bounds = skel_joint_bounds(&synth_buffer(&SAMPLES), 3);

        let mut joints = FfxiJointMatrices::default();
        joints.matrices[0] = Mat4::from_scale_rotation_translation(
            Vec3::splat(1.2),
            Quat::from_rotation_y(0.7),
            Vec3::new(0.3, 1.1, -0.2),
        );
        joints.matrices[1] =
            Mat4::from_rotation_translation(Quat::from_rotation_x(-0.4), Vec3::new(-0.5, 0.8, 0.6));
        joints.matrices[2] =
            Mat4::from_rotation_translation(Quat::from_rotation_z(1.9), Vec3::new(0.0, 0.4, 1.3));

        let aabb = entity_aabb_from_joints(&joints, &bounds).expect("bounds from skinned verts");
        let lo = Vec3::from(aabb.min());
        let hi = Vec3::from(aabb.max());
        for &(q0, q1, w, j0, j1) in &SAMPLES {
            let m0 = joints.matrices[j0 as usize];
            let m1 = joints.matrices[j1 as usize];
            let p = (m0 * (q0 * w).extend(w) + m1 * (q1 * (1.0 - w)).extend(1.0 - w)).truncate();
            assert!(
                p.cmpge(lo).all() && p.cmple(hi).all(),
                "skinned position {p} escapes the joint-bound union [{lo}, {hi}]"
            );
        }
    }

    #[test]
    fn actor_children_spawn_with_manual_aabbs() {
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<FfxiSkinnedMaterial>::default();
        let mut cache = FfxiSkinnedMaterialCache::default();
        let mut registry = FfxiSkinRegistry::default();
        let mut images = Assets::<Image>::default();

        let loaded = LoadedActor {
            skeleton: Arc::new(Skeleton {
                id: DatId::from_str("0000"),
                joints: Vec::new(),
                references: Vec::new(),
                bounding_boxes: Vec::new(),
            }),
            skel_meshes: Vec::new(),
            effect_meshes: Vec::new(),
            textures: Vec::new(),
            animations: Arc::new(Vec::new()),
            battle_clips: Arc::new(Vec::new()),
            routines: Arc::new(HashMap::new()),
            action_assets: Arc::new(crate::scheduler_runtime::ActionAssets::default()),
            rejected_clips: Vec::new(),
            model_dat: "test.DAT".to_string(),
            cib: None,
        };
        let buffer = synth_buffer(&SAMPLES);
        let parts = PreparedParts {
            images: Vec::new(),
            skel_built: vec![BuiltGroup {
                mesh: build_mesh(&buffer, 3),
                texture_name: String::new(),
                tint: Vec4::ONE,
                joint_aabbs: skel_joint_bounds(&buffer, 3),
            }],
            d3m_built: Vec::new(),
            bind_joints: FfxiJointMatrices::default(),
            bounds: None,
        };
        let mesh_handles = add_part_meshes(&parts, &mut meshes);
        let skin_slot = registry.alloc_skin();

        let mut state: bevy::ecs::system::SystemState<Commands> =
            bevy::ecs::system::SystemState::new(&mut world);
        let mut commands = state.get_mut(&mut world).expect("commands param");
        let root = commands.spawn_empty().id();
        build_actor_children(
            &mut commands,
            &mesh_handles,
            &mut materials,
            &mut cache,
            &mut registry,
            &mut images,
            &loaded,
            &parts,
            root,
            skin_slot,
        );
        state.apply(&mut world);

        let mut q = world.query_filtered::<(Option<&Aabb>, Option<&ActorMeshJointBounds>), With<FfxiActorMeshChild>>();
        let children: Vec<_> = q.iter(&world).collect();
        assert!(!children.is_empty(), "spawn must produce submesh children");
        for (aabb, bounds) in children {
            assert!(
                aabb.is_some(),
                "actor submesh children must carry an Aabb or every submesh is drawn \
                 in the main pass, prepass, and each shadow cascade regardless of frustum"
            );
            assert!(
                bounds.is_some(),
                "actor submesh children must carry ActorMeshJointBounds so \
                 update_actor_mesh_aabbs keeps the Aabb tracking the pose"
            );
        }
    }
}

#[cfg(test)]
mod motion_dat_tests {
    use super::*;
    use ffxi_dat::resource_dir::ResourceDir;

    fn clip_joints(root: &DatRoot, file_id: u32, prefix: &str) -> Option<Vec<u32>> {
        let bytes = read_dat(root, file_id)?;
        ResourceDir::from_bytes(bytes)
            .collect_animations()
            .iter()
            .find(|a| a.id.as_str().starts_with(prefix))
            .map(|a| {
                let mut j: Vec<u32> = a.key_frame_sets.keys().copied().collect();
                j.sort_unstable();
                j
            })
    }

    // Retail-byte guard (skips without an install). The three motion DATs around
    // the race base drive disjoint joint ranges, so dropping the waist set leaves
    // its joints in bind pose rather than degrading gracefully.
    #[test]
    fn real_dat_waist_motion_covers_joints_no_other_set_touches() {
        let Ok(root) = DatRoot::from_env_or_default() else {
            return;
        };
        let Some(base) = skeleton_file_id_for_race(1) else {
            return;
        };
        let Some(upper) = clip_joints(&root, base + UPPER_BODY_MOTION_OFFSET, "wlk") else {
            return;
        };
        let waist = clip_joints(
            &root,
            base + u32::from(WAIST_TYPE_MIN) + WAIST_MOTION_OFFSET,
            "wlk",
        )
        .expect("waist motion DAT ships a walk clip");

        assert!(!waist.is_empty(), "waist clip drives no joints");
        assert!(
            waist.iter().all(|j| !upper.contains(j)),
            "waist joints {waist:?} overlap upper-body joints {upper:?}"
        );
    }

    // The two waist variants are different sets, not duplicates -- which is why
    // the selector has to come from the body armour's CIB instead of a fixed +3.
    #[test]
    fn real_dat_waist_variants_differ_for_some_race() {
        let Ok(root) = DatRoot::from_env_or_default() else {
            return;
        };
        let differs = (1u8..=8).any(|race| {
            let Some(base) = skeleton_file_id_for_race(race) else {
                return false;
            };
            let count = |off: u32| {
                read_dat(&root, base + off)
                    .map(|b| ResourceDir::from_bytes(b).collect_animations().len())
                    .unwrap_or(0)
            };
            let a = count(WAIST_MOTION_OFFSET + 1);
            let b = count(WAIST_MOTION_OFFSET + 2);
            a > 0 && b > 0 && a != b
        });
        assert!(
            differs,
            "no race distinguishes the two waist variants -- selector may be moot"
        );
    }
}

#[cfg(test)]
mod clip_warn_tests {
    use super::*;

    // The dedupe key is (world_id, clip, reason): one CLIP_WARN line per entity per requested
    // clip per diagnostic. Unique ids keep this test independent of any other test in the
    // process sharing the global seen-set.
    #[test]
    fn clip_warn_prints_once_per_pair() {
        let id_a = 0xC0DE_0001;
        let id_b = 0xC0DE_0002;
        let wlk = DatId::from_str("wlk?");

        assert!(clip_warn_once(id_a, "ROM/4/999.DAT", &wlk, "not_found"));
        assert!(
            !clip_warn_once(id_a, "ROM/4/999.DAT", &wlk, "not_found"),
            "second sighting of the same pair stays quiet"
        );
        assert!(
            clip_warn_once(id_a, "ROM/4/999.DAT", &wlk, "seq_load_error"),
            "a different reason on the same pair gets its own line"
        );
        assert!(
            clip_warn_once(id_b, "ROM/4/999.DAT", &wlk, "not_found"),
            "a different entity id gets its own line"
        );
    }
}
