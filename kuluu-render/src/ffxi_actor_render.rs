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

use crate::look_at_gates::{self as look_at_gates, LookAtLockInterval, LookAtLockTask};
use ffxi_actor::actor_state::{self, ActorAnimInputs, RestKind};
use ffxi_actor::animation::{
    BonePoseScratch, LoopParams, SkeletonAnimationCoordinator, TransitionParams,
};
use ffxi_actor::look_bend::apply_look_bends;
use ffxi_actor::skeleton_instance::{
    pose_world, pose_world_mounted_into, standard_joint_world_position, MountAttach, PoseScratch,
    RootTransform,
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
use crate::scheduler_runtime::main_dll_for_root;
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
        /// The entity's `Flags1.GraphSize`, which picks one of the model's four
        /// authored CIB scales (research/XIClient/src/XIClient/source/World/
        /// Actor/SkeletalMeshActor.cpp `SkeletalMeshActor::GetCibScaleIndex`).
        graph_size: u8,
    },

    /// A ridden mount whose model is a PC race config rather than an NPC model.
    /// Only the chocobo is built this way in retail — one race per coat colour,
    /// with the body parts coming from the equipment table like a PC's gear.
    /// research/xim poc/Model.kt, RaceGenderConfig.
    Mount { race: u8 },
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

/// Tick counter for the gated hold probe below; advanced once per snapshot tick in
/// `tick_live_ffxi_actors` (serial section), read from the parallel pose pass.
static SPECIAL_LOG_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// CLIP_WARN is a tracing::debug! event on target "clip": a selected clip that resolves to
/// nothing in the model DAT gets one line (a frozen mob otherwise announces itself nowhere),
/// visible under RUST_LOG=clip or =debug. KULUU_CLIP_LOG additionally prints CLIP_OK for
/// successful resolutions, which is chatty enough to stay gated.
fn clip_log_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::env_flags::env_flag(&ENABLED, "KULUU_CLIP_LOG")
}

/// KULUU_ANIM_LOG traces one pose step at a time for one actor (the client-controlled one): the
/// selection decision and every active layer's playhead before/after `coordinator.update`. A
/// playhead that lands below its previous value kept its slot and restarted, which is what reads
/// on screen as an animation repeating itself.
static ANIM_TRACE_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn anim_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::env_flags::env_flag(&ENABLED, "KULUU_ANIM_LOG")
}

fn anim_traced(id: u32) -> bool {
    anim_trace_enabled() && ANIM_TRACE_ID.load(std::sync::atomic::Ordering::Relaxed) == id
}

/// `slot:clip:frame` per active layer, `+t` marking a running crossfade.
fn anim_playheads(coordinator: &SkeletonAnimationCoordinator) -> String {
    let mut out = String::new();
    for (slot, animator) in coordinator.animations.iter().enumerate() {
        let Some(ctx) = animator.as_ref().and_then(|a| a.current_animation.as_ref()) else {
            continue;
        };
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                " {}:{}:{:.2}{}",
                slot,
                ctx.animation.id.as_str(),
                ctx.current_frame,
                if animator.as_ref().is_some_and(|a| a.transition.is_some()) {
                    "+t"
                } else {
                    ""
                }
            ),
        );
    }
    out
}

/// The wire's animation byte, logged whenever it changes (KULUU_ANIM_LOG): which draws and
/// sheathes self is decided by that byte, so a repeat of either needs to say whether the byte
/// moved or kuluu's own reading of it did.
fn log_self_server_status_change(status: u8, drawn: bool) {
    static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0xFFFF_FFFF);
    if !anim_trace_enabled() {
        return;
    }
    let previous = LAST.swap(status as u32, std::sync::atomic::Ordering::Relaxed);
    if previous != status as u32 {
        tracing::info!(
            target: "clip",
            "ANIM_STATUS self_server_status {previous:#04x} -> {status:#04x} drawn={drawn}"
        );
    }
}

/// Once-per-(world_id, clip, reason) dedupe for CLIP_WARN: a miss repeats every frame while the
/// pose is held, and the diagnosis only needs the first sighting per entity per requested clip.
/// The reason stays in the key so two distinct diagnostics on one pair (a not_found that also
/// leaves current_clip untouched) both get their line instead of deduping into one.
static CLIP_WARN_SEEN: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<(u32, String, &'static str)>>,
> = std::sync::OnceLock::new();

/// Returns true when this call printed the line (first sighting of the pair), false when the
/// dedupe set already had it. Callers do not branch on it; tests use it to pin the dedupe key.
fn clip_warn_once(id: u32, name: &str, model: &str, clip: &DatId, reason: &'static str) -> bool {
    let seen = CLIP_WARN_SEEN.get_or_init(Default::default);
    let Ok(mut guard) = seen.lock() else {
        return false;
    };
    if !guard.insert((id, clip.as_str(), reason)) {
        return false;
    }
    tracing::debug!(
        target: "clip",
        "CLIP_WARN id={id:#x} name={} model={} clip={} reason={}",
        name,
        model,
        clip.as_str(),
        reason
    );
    true
}

/// CLIP_WARN reason for a tier whose requested clip resolved to no usable chunk:
/// seq_load_error when the model's DAT walk saw the chunk but parse rejected it,
/// not_found_override_skipped when an override tier (special/fishing) asked for a clip the model
/// does not ship, and not_found otherwise. The reason stays in the dedupe key so two distinct
/// misses on one pair both print.
fn clip_miss(
    id: u32,
    name: &str,
    model: &str,
    clip: &DatId,
    rejected_clips: &[DatId],
    tier: PoseTier,
) {
    let reason = if rejected_clips.iter().any(|r| r.parameterized_match(clip)) {
        "seq_load_error"
    } else if matches!(tier, PoseTier::Special | PoseTier::Fishing) {
        "not_found_override_skipped"
    } else {
        "not_found"
    };
    clip_warn_once(id, name, model, clip, reason);
}

/// CLIP_OK under RUST_LOG=clip=debug: the requested clip actually resolved; one line per
/// transition into current_clip, naming the chunk that won and its frame count.
fn clip_ok(id: u32, asked: &DatId, resolved: &SkeletonAnimation, movement_type: MovementType) {
    if !clip_log_enabled() {
        return;
    }
    tracing::debug!(
        target: "clip",
        "CLIP_OK id={id:#x} clip={} -> {} frames={} move={}",
        asked.as_str(),
        resolved.id.as_str(),
        resolved.num_frames,
        movement_type
    );
}

pub(crate) fn ffxi_to_bevy_basis() -> Quat {
    Quat::from_rotation_x(std::f32::consts::PI)
}

#[derive(Clone)]
struct NamedTexture {
    name: String,
    texture: DecodedTexture,
}

fn split_actor_textures(
    textures: Vec<NamedTexture>,
    q: crate::zone_texture::TextureQuality,
) -> (Vec<String>, Vec<Image>) {
    textures
        .into_iter()
        .map(|nt| (nt.name, decoded_texture_to_image(nt.texture, q)))
        .unzip()
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

    /// Named-play routines present in this model's DAT walk whose scheduler parse was rejected;
    /// CLIP_WARN must tell "the routine chunk is there but broken" (seq_load_error) apart from
    /// "the model ships no such routine at all" (not_found). The reason itself lives on
    /// `LoadedActor.rejected_routines`.
    rejected_routines: Vec<ffxi_dat::resource_dir::RejectedRoutine>,

    /// The primary model DAT as `{rom_dir}/{dir}/{file}.DAT` (e.g. ROM/4/109.DAT), for the
    /// CLIP_WARN line's `model=` field.
    model_dat: String,

    /// The model's Cib Info chunk when its primary DAT carries one (vekien/xi-model-viewer
    /// ui/js/dat/inspect.js parseInspectInfo). Mounts are None on purpose: their Info layout is
    /// the `mount` variant, whose +0x0A byte is a pose type, not a scale (research/xim
    /// resource/InfoSection.kt readMountDefinition).
    cib: Option<Cib>,

    /// `(handle joint, hand bone)` pairs to re-parent while the actor is display-engaged; see
    /// [`weapon_handle_overrides`]. Main-hand first, then sub.
    pub weapon_handles: Vec<(usize, usize)>,
}

/// A parsed motion chunk the pose pass can actually play: at least one key frame set and at
/// least two frames. `skel_anim::parse` reports bad data as an empty SkeletonAnimation, so a
/// zero-frame or joint-less entry in `animations` is a parse rejection, not a clip the model
/// ships.
fn is_usable_clip(anim: &SkeletonAnimation) -> bool {
    !anim.key_frame_sets.is_empty() && anim.num_frames > 0
}

/// Clip/scheduler parsing is the expensive tail of an actor load; deriving it here keeps it on
/// the loader task instead of the render main thread, and the Arcs let consumers share the
/// parsed sets without deep-cloning keyframe data. The fourth and fifth return values list the
/// chunks seen in the DAT walk but rejected by parse, so CLIP_WARN can name a seq_load_error
/// instead of a not_found.
fn derive_animation_sets(
    anim_dirs: &[ResourceDir],
    battle_dirs: &[ResourceDir],
) -> (
    Arc<Vec<SkeletonAnimation>>,
    Arc<Vec<SkeletonAnimation>>,
    Arc<HashMap<DatId, Scheduler>>,
    Vec<DatId>,
    Vec<ffxi_dat::resource_dir::RejectedRoutine>,
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
    let mut rejected_routines: Vec<ffxi_dat::resource_dir::RejectedRoutine> = Vec::new();
    for dir in battle_dirs.iter().chain(anim_dirs.iter()) {
        let (scheds, rejected) = dir.collect_schedulers_with_rejections();
        rejected_routines.extend(rejected);
        for sched in scheds {
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
        rejected_routines,
    )
}

/// The CLIP_WARN `model=` label for a file id: `{rom_dir}/{dir}/{file}.DAT`, the same join as
/// `DatLocation::join_under` without the install root.
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
    texture_names: Vec<String>,

    skel_built: Vec<BuiltGroup>,

    d3m_built: Vec<BuiltGroup>,

    bind_joints: FfxiJointMatrices,

    /// Bind-pose bounds of the assembled actor in bevy space (feet at y≈0),
    /// computed with the same facing/scale as `bind_joints` — i.e. the mesh as
    /// drawn. Feeds camera and hitbox bounds.
    pub bounds: Option<(Vec3, Vec3)>,
}

/// A finished load task's product. The images sit outside [`PreparedActor`]
/// because the spawn path `Arc`-wraps that, and a `Vec` cannot move out of an `Arc`.
struct PreparedLoad {
    actor: PreparedActor,
    images: Vec<Image>,
}

pub struct PreparedActor {
    pub loaded: LoadedActor,
    parts: PreparedParts,

    /// The model's transform scale, resolved once at load time. 1.0 for PCs and mounts;
    /// the Cib Info `scale` byte divided by 100 for NPC models (see kick_load_actor_tasks).
    pub scale: f32,
}

fn prepare_actor_parts(
    loaded: &LoadedActor,
    texture_names: Vec<String>,
    facing_dir: f32,
    scale: f32,
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

    PreparedParts {
        texture_names,
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
        graph_size: u8,
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
        ActorSubject::Npc {
            file_id,
            graph_size,
        } => ActorPrepKey::Npc {
            file_id: *file_id,
            graph_size: *graph_size,
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
/// Bound idle HD looks without forcing every ordinary look to reload.
const ACTOR_PREP_CACHE_BYTES: usize = 128 * 1024 * 1024;

struct ActorPrepEntry {
    prepared: Arc<PreparedActor>,
    /// Filled on first spawn: every later spawn of this look reuses the same Mesh assets, so
    /// Bevy's batcher can group their draws (same pipeline + material + mesh) instead of
    /// encoding one draw per fresh Mesh handle.
    mesh_handles: Vec<Handle<Mesh>>,
    /// Uploaded once per look at load completion, so N entities sharing a look cost one GPU
    /// texture set and one material rather than N of each.
    image_handles: Vec<Handle<Image>>,
    image_bytes: usize,
}

#[derive(Default)]
struct ActorPrepCache {
    map: HashMap<ActorPrepKey, ActorPrepEntry>,
    order: std::collections::VecDeque<ActorPrepKey>,
}

impl ActorPrepCache {
    fn get_and_promote(
        &mut self,
        key: &ActorPrepKey,
    ) -> Option<(Arc<PreparedActor>, Vec<Handle<Image>>)> {
        let entry = self.map.get(key)?;
        let hit = (Arc::clone(&entry.prepared), entry.image_handles.clone());
        self.order.retain(|k| k != key);
        self.order.push_back(key.clone());
        Some(hit)
    }

    fn insert(
        &mut self,
        key: ActorPrepKey,
        prepared: Arc<PreparedActor>,
        image_handles: Vec<Handle<Image>>,
        image_bytes: usize,
    ) {
        let entry = ActorPrepEntry {
            prepared,
            mesh_handles: Vec::new(),
            image_handles,
            image_bytes,
        };
        if self.map.insert(key.clone(), entry).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > ACTOR_PREP_CACHE_CAP
            || self
                .map
                .values()
                .map(|entry| entry.image_bytes)
                .sum::<usize>()
                > ACTOR_PREP_CACHE_BYTES
        {
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

// Every PC/NPC/mount load resolves through one shared root: `DatRoot::open` re-runs overlay
// discovery and the FFXiMain.dll SHA-256 client-profile probe, so opening one per actor spawn is
// pure repeat work competing with the render task pool. Wired by kuluu's `insert_dat_roots` like
// every other `*DatRoot` (see `scheduler_runtime::ActionDatRoot`).
#[derive(Resource, Default, Clone)]
pub struct ActorDatRoot(pub Option<Arc<DatRoot>>);

pub(crate) fn resolve_actor_root(wired: Option<Arc<DatRoot>>) -> Result<Arc<DatRoot>, String> {
    wired.ok_or_else(|| "no DAT root wired".to_string())
}

pub fn load_npc(root: &DatRoot, file_id: u32) -> Result<LoadedActor, String> {
    crate::perf_probe::note_model_load();
    let bytes = read_dat(root, file_id).ok_or_else(|| format!("read npc dat {file_id}"))?;

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

    let (_schedulers, action_assets, _cameras) =
        crate::scheduler_runtime::parse_action_bytes(&bytes);
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
    let (animations, battle_clips, routines, rejected_clips, rejected_routines) =
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
        rejected_routines,
        model_dat: model_dat_label(root, file_id),
        cib: dir.first_cib(),
        weapon_handles: Vec::new(),
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
pub fn load_mount_race(root: &DatRoot, race: u8) -> Result<LoadedActor, String> {
    crate::perf_probe::note_model_load();
    let dll = main_dll_for_root(root.root())
        .ok_or_else(|| format!("FFXiMain.dll unreadable under {}", root.root().display()))?;
    let table_index = mount_equipment_table_index(race)
        .ok_or_else(|| format!("race {race} is not a mount race config"))?;
    let skel_file_id = u32::from(
        dll.base_race_config_index(race)
            .ok_or_else(|| format!("no race-config table entry for mount race {race}"))?,
    );

    let skel_bytes = read_dat(root, skel_file_id)
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
        let Some(bytes) = read_dat(root, file_id) else {
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

    let (animations, battle_clips, routines, rejected_clips, rejected_routines) =
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
        rejected_routines,
        model_dat: model_dat_label(root, skel_file_id),
        // The mount's own Info chunk uses the `mount` layout (rotation/poseType at +0x02/+0x0A,
        // research/xim resource/InfoSection.kt readMountDefinition); parsing it with the info
        // layout would misread poseType as a scale byte, so mounts carry no CIB.
        cib: None,
        weapon_handles: Vec::new(),
    })
}

/// The naked default body: face 0 and model 0 of each clothing slot (1 head
/// .. 5 feet); the weapon slots stay empty.
fn default_pc_equipment(dll: &ffxi_dat::main_dll::MainDll, race: u8) -> Vec<u32> {
    use crate::look_resolver::{equipment_dat_id, face_dat_id};
    let mut out = Vec::new();
    if let Some(f) = face_dat_id(dll, 0, race) {
        out.push(f);
    }

    for slot in 1u8..=5 {
        if let Some(f) = equipment_dat_id(dll, slot, 0, race) {
            out.push(f);
        }
    }
    out
}

// research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp
// SkeletalMeshActor::GetUpperBodyDatIndex and SkeletalMeshActor::GetWaistDatIndex
// — the two companion motion DATs sit at fixed offsets from the race skeleton
// base, indexed by a CIB byte.
const CIB_MOTION_INDEX_NONE: u8 = 0xFF;
const UPPER_BODY_MOTION_OFFSET: u32 = 1;
const WAIST_MOTION_OFFSET: u32 = 2;
/// `SkeletalMeshActor::GetWaistDatIndex` floors waist_type at 1 before using it, so an
/// unequipped or CIB-less body still resolves to the trousers waist rather than colliding with
/// the upper-body DAT.
const WAIST_TYPE_MIN: u8 = 1;

/// One DAT off the race's action-animation base.
/// research/xim poc/Model.kt, PcModel.getMountAnimationResource.
fn action_anim_dat(
    root: &DatRoot,
    dll: Option<&ffxi_dat::main_dll::MainDll>,
    race: u8,
    offset: u16,
) -> Option<Vec<u8>> {
    let base = dll?.base_action_animation_index(race)?;
    read_dat(root, u32::from(base + offset))
}

/// The `(handle joint, hand bone)` pairs retail re-parents while a PC is display-engaged
/// (research/xim resource/SkeletonInstance.kt computeJointParentOverrides): each equipped weapon names
/// its handle through its Info standard-joint byte resolved in this skeleton's reference table, and the
/// pair is that joint with the bone the hand standard position names. A held mesh binds to a child of
/// the handle joint, so without the re-parent it rides the hip chain - rotating with every torso step -
/// instead of following the arm.
fn weapon_handle_overrides(
    skeleton: &Skeleton,
    main_weapon: Option<&Cib>,
    sub_weapon: Option<&Cib>,
) -> Vec<(usize, usize)> {
    let mut handles = Vec::new();
    for (weapon, hand_slot) in [
        (main_weapon, ffxi_dat::skel::standard_position::RIGHT_HAND),
        (sub_weapon, ffxi_dat::skel::standard_position::LEFT_HAND),
    ] {
        let Some(slot) = weapon.and_then(|c| c.standard_joint).map(usize::from) else {
            continue;
        };
        let (Some(handle), Some(hand)) = (
            skeleton.reference_at(slot),
            skeleton.reference_at(hand_slot),
        ) else {
            continue;
        };
        if handle.index != hand.index && !handles.contains(&(handle.index, hand.index)) {
            handles.push((handle.index, hand.index));
        }
    }
    handles
}

pub fn load_pc(
    root: &DatRoot,
    race: u8,
    mounted: bool,
    equipment: &[u32],
    body: Option<u32>,
    main_weapon: Option<u32>,
    sub_weapon: Option<u32>,
) -> Result<LoadedActor, String> {
    // One parsed FFXiMain.dll per install root, shared with every other consumer;
    // `None` (unreadable) degrades to the shipped fallback tables for the
    // skeleton and battle DATs and to no action poses or default body.
    let dll = main_dll_for_root(root.root());
    if dll.is_none() {
        warn!("load_pc race={race}: FFXiMain.dll unreadable; action poses and default gear unavailable");
    }
    let skel_file_id = skeleton_file_id_for_race(dll.as_deref(), race)
        .ok_or_else(|| format!("unsupported race {race}"))?;

    let skel_bytes =
        read_dat(root, skel_file_id).ok_or_else(|| format!("read skel dat {skel_file_id}"))?;
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
    // `base + is_shield + 1` (SkeletalMeshActor.cpp
    // SkeletalMeshActor::GetUpperBodyDatIndex) — a shield swaps in a variant with
    // its own joint count. Waist/skirt is `base + max(waist_type, 1) + 2`
    // (SkeletalMeshActor::GetWaistDatIndex, reached from
    // SkeletalMeshActor::ReadStdMotionRes), which drives the hip-hung cloth
    // joints; without it they hold bind pose through every idle, walk, run,
    // strafe and death.
    //
    // Both selectors come from equipment CIBs and neither is a fixed offset: the
    // sub slot supplies is_shield and the body slot waist_type
    // (SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel collects them per
    // slot). Loading a fixed `+3` instead would give every robed mage trouser
    // motion, and because `dedup_clips` is first-writer-wins, loading both
    // candidates would silently keep whichever came first rather than the
    // authored one.
    let cib_byte = |file_id: Option<u32>, pick: fn(&ffxi_dat::cib::Cib) -> u8| {
        file_id
            .and_then(|f| read_dat(root, f))
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
        if let Some(bytes) = read_dat(root, skel_file_id + offset) {
            anim_dirs.push(ResourceDir::from_bytes(bytes));
        }
    }

    // The seat poses (`chi?` for a chocobo, `{n}un?` for the other mounts) plus
    // their own run/walk variants ship in one DAT off the race's action-animation
    // base, and only while riding does retail put it in the animation set.
    if mounted {
        match action_anim_dat(
            root,
            dll.as_deref(),
            race,
            ffxi_dat::main_dll::ACTION_ANIM_MOUNT_OFFSET,
        ) {
            Some(bytes) => anim_dirs.insert(0, ResourceDir::from_bytes(bytes)),
            None => warn!("load_pc race={race}: no mount-pose DAT — rider will not sit"),
        }
    }

    // Fishing is not a load-time property the way mounting is — any PC in view
    // can start a cast at any time, and an actor is built once — so the fishing
    // DAT rides along for every PC. It is a quarter the size of the race base
    // and carries the `fsh*` routines the pose selector resolves through.
    match action_anim_dat(
        root,
        dll.as_deref(),
        race,
        ffxi_dat::main_dll::ACTION_ANIM_FISHING_OFFSET,
    ) {
        Some(bytes) => anim_dirs.push(ResourceDir::from_bytes(bytes)),
        None => warn!("load_pc race={race}: no fishing DAT — fishing poses unavailable"),
    }

    let resolved_default;
    let equipment = if equipment.is_empty() {
        resolved_default = dll
            .as_deref()
            .map(|dll| default_pc_equipment(dll, race))
            .unwrap_or_default();
        resolved_default.as_slice()
    } else {
        equipment
    };

    let mut equip_trace: Vec<(u32, &'static str)> = Vec::new();
    for &file_id in equipment {
        let Some(bytes) = read_dat(root, file_id) else {
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

    let weapon_cib = |file: Option<u32>| {
        file.and_then(|f| read_dat(root, f))
            .map(ResourceDir::from_bytes)
            .and_then(|d| d.first_cib())
    };
    let main_weapon_cib = weapon_cib(main_weapon);
    let sub_weapon_cib = weapon_cib(sub_weapon);
    let weapon_anim_type = main_weapon_cib.map(|c| c.motion_index).unwrap_or(0);
    // The sub slot's own animation byte is what makes a rig dual-wield, and it selects the off-hand
    // block too (research/xim poc/Model.kt PcModel.isDualWield).
    let sub_weapon_anim_type = sub_weapon_cib.map(|c| c.motion_index);
    let weapon_handles =
        weapon_handle_overrides(&skeleton, main_weapon_cib.as_ref(), sub_weapon_cib.as_ref());
    let mut battle_dirs = Vec::new();
    if let Some(base) = combat_stance::motion_dat_for_race(dll.as_deref(), race) {
        // One weapon pairing block per actor in the body's motion pool: `battle base + main-hand byte`, or
        // that pairing's dual-wield main-hand base. The off-hand and waist/cloth blocks from the same tables
        // stay out: they author a new member of an existing id family (files 9928/9929/9930 carry `btl2` next
        // to at02/at12/at22 where race-wide 9672 carries btl0/btl1) and pose_clip_matches registers every clip
        // matching a parameterized id as a simultaneous layer, so merging them poses cloth motion over the
        // battle stance and moves an equipped mesh off its hand. They need their own masked slot (gaps §G.12).
        let stance_block = combat_stance::BattleMotionBlocks::resolve(
            dll.as_deref(),
            race,
            weapon_anim_type,
            sub_weapon_anim_type,
        )
        .map(|blocks| blocks.main);
        if weapon_anim_type != 0 && weapon_anim_type != CIB_MOTION_INDEX_NONE {
            let file = stance_block.unwrap_or(base + u32::from(weapon_anim_type));
            if let Some(dir) = read_dat(root, file)
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

        if let Some(dir) = read_dat(root, base).map(ResourceDir::from_bytes) {
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

    let (animations, battle_clips, routines, rejected_clips, rejected_routines) =
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
        rejected_routines,
        model_dat: model_dat_label(root, skel_file_id),
        cib: race_cib,
        weapon_handles,
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
    rejected_routines: Vec<ffxi_dat::resource_dir::RejectedRoutine>,
    model_dat: String,
    coordinator: SkeletonAnimationCoordinator,
    skin_slot: u32,
    instance_slots: Vec<u32>,

    pub inputs: ActorAnimInputs,

    pub world_id: u32,

    pub facing_dir: f32,

    pub scale: f32,

    /// The model's Cib Info movement byte (Unset when the DAT carries no CIB). Gates whether
    /// the wire AnimationSpeed stride scale applies to locomotion clip playback.
    movement_type: MovementType,

    /// The model's Cib Info waist byte (0 when the DAT carries no CIB): which of a Tpc motion
    /// package's two tag-2 containers the renderer loads.
    body_armour_waist: u8,

    current_clip: Option<(DatId, bool)>,

    /// The registered clip per slot, `(id, battle set)`: when the selection key changes, only
    /// slots whose clip actually changed re-register; an unchanged slot keeps its frame, the
    /// same-clip re-request no-op of research/xim SkeletonAnimator.kt setNextAnimation.
    registered_slots: [Option<(DatId, bool)>; 8],

    /// Idle-family requests the coordinator declined (its handover gate is per-animator), so they can
    /// be re-issued until one lands. Without this a declined request stays recorded in
    /// `registered_slots` and is never asked again.
    pending_idle_registrations: [Option<(DatId, bool)>; 8],

    rest_phase: RestPlayback,

    death_phase: actor_state::DeathPhase,

    engage: EngageMachine,

    knockback: Option<KnockbackPlayback>,

    action: Option<ActionPlayback>,
    event_idle: Option<DatId>,
    action_clips: Vec<SkeletonAnimation>,

    head_look: HeadLook,

    /// Locked on and moving this frame (self only): the walker aimed the body at the target, so a side step's
    /// look-at measures from the chest and its spine blends come round through the front ([`crate::locked_torso`]).
    pub locked_on: bool,

    /// How far that applies; it ramps over the locomotion crossfade the side-step clip itself fades in on.
    locked_torso_weight: f32,

    /// The chest axis and spine chain of this rig; `None` for a rig without chest, neck and feet references.
    upper_body: Option<crate::locked_torso::UpperBody>,

    /// The entity's wire animation byte this frame — the byte retail reads for both of its look-at
    /// gates and for the bend's record count (crate::look_at_gates).
    wire_animation: u8,

    /// The motion-id families this actor's routines have switched it to (`StageKind::AnimationMode`;
    /// research/xim EffectRoutineInstance.kt handleAdjustAnimationModeRoutine keeps these per model, and
    /// `Actor.kt getAnimationModeVariant` resolves every later lookup as `<variant><family>?` with the
    /// unmarked id kept as fallback). Weapon anchors are only keyed by their family's clips, so this is
    /// what puts a drawn weapon on its authored motion group.
    anim_modes: ActorAnimModes,

    /// This actor's live `0x89` LockLookAt tasks, ticked against the routine clocks each frame.
    look_at_tasks: Vec<LookAtLockTask>,

    /// Retail's per-actor visibility/status WORD (`FFXiMain.dll retail-2026-09`: zeroed in the actor
    /// constructor at RVA 0x81C3A; a whole-word test gates the look-point lowering at RVA 0xD5C7A).
    /// The bits come from systems kuluu does not run — staggered visibility/occlusion (setters at
    /// RVA 0xCC6AE, RVA 0xCD032, RVA 0xCBDBC; cleared by the draw-list sweep at RVA 0x82D5E),
    /// class-swap re-init (RVA 0xC5D60) and event state machines — so an actor holds retail's
    /// constructor default of zero unless a kuluu system that models one of those lands.
    look_at_visibility_word: u16,

    pub last_clip: Option<DatId>,
    pub last_frame: f32,

    world_pose: Vec<Mat4>,
    pose_work: PoseScratch,

    /// This frame's record per joint, rebuilt from the live layers each tick: a joint no active clip
    /// keys holds nothing and composes from its own bind data (research/xim
    /// resource/SkeletonInstance.kt updateCurrentJointTransform). Kept on the actor only so the record
    /// buffer is not reallocated per frame.
    pose_scratch: BonePoseScratch,

    /// The `(handle joint, hand bone)` pairs this actor's equipped weapons resolve to; empty unless
    /// the model loaded with a weapon naming a handle. Applied while display-engaged - see
    /// [`weapon_handle_overrides`].
    weapon_handles: Vec<(usize, usize)>,

    point_light_selection: Option<ActorPointLightSelection>,
}

impl FfxiRenderActor {
    pub fn skin_slot(&self) -> u32 {
        self.skin_slot
    }

    /// Applies one animation-mode stage fired by this actor's routines: its motion ids resolve against
    /// that variant family from here on.
    pub fn set_animation_mode(&mut self, slot: ffxi_dat::scheduler::AnimModeSlot, variant: u8) {
        self.anim_modes.set(slot, variant);
    }

    /// The families this actor's routines have switched it to.
    pub fn anim_modes(&self) -> ActorAnimModes {
        self.anim_modes
    }

    pub fn world_pose(&self) -> &[Mat4] {
        &self.world_pose
    }

    /// The clip id the pose pass currently has selected (None before its first run).
    pub fn current_clip_id(&self) -> Option<&DatId> {
        self.current_clip.as_ref().map(|(id, _)| id)
    }

    /// The Cib Info movement byte this model was loaded with (Unset when the DAT carries no
    /// CIB); gates the wire stride scale on locomotion clip playback.
    pub fn movement_type(&self) -> MovementType {
        self.movement_type
    }

    /// The Cib Info waist byte this model was loaded with (0 when the DAT carries no CIB):
    /// which of a Tpc motion package's two tag-2 containers the renderer loads.
    pub fn body_armour_waist(&self) -> u8 {
        self.body_armour_waist
    }

    /// The completion motion's clip while `action` is held - what a routine's Motion stage (or
    /// the flinch consumer) just started. None when no completion motion owns the pose.
    pub fn active_action_clip(&self) -> Option<&DatId> {
        self.action.as_ref().map(|a| &a.clip_id)
    }

    /// Drop the completion motion a cutscene cast started so the pose falls back to idle on the
    /// next frame. A cutscene's cast pose (the gate guard's Signet arm-raise) is owned by the
    /// event, not by combat: when the event ends, the pose must not outlive it. The pose pass
    /// re-selects idle from the cleared `action` on its next run.
    pub fn clear_cutscene_action(&mut self) {
        self.event_idle = None;
        self.action = None;
        self.action_clips.clear();
        self.coordinator.clear();
        self.current_clip = None;
    }

    /// A scheduler Motion stage's action is in flight on this model.
    pub fn has_action(&self) -> bool {
        self.action.is_some()
    }

    pub fn instance_slots(&self) -> &[u32] {
        &self.instance_slots
    }

    // pub (not pub(crate)): the AnimationTest box reads an actor's own routine table to merge it
    // into its lookups.
    pub fn routines(&self) -> &HashMap<DatId, Scheduler> {
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

    /// The weapon is mid draw or mid sheathe: retail holds the player in
    /// place for the transition (record:
    /// .agents/skills/retail-observe/references/2026-09-21-action-confirm-and-locks.md,
    /// "The one real player lock is the weapon draw and sheathe").
    pub fn engage_transition_in_progress(&self) -> bool {
        matches!(
            self.engage,
            EngageMachine::Drawing { .. } | EngageMachine::Sheathing { .. }
        )
    }

    /// Starts a knockback on this actor (research/xim
    /// EffectRoutineInterpolatedEffects.kt KnockBackInstance init): the
    /// knock-down clip for `animation_frames`, the lock for the whole run. It
    /// deliberately consults neither the pose-idle nor the scheduler lock a
    /// flinch checks: a knockback lands over a swing, a cast pose or the draw.
    pub fn begin_knockback(&mut self, dir: Vec2, level: u8, animation_frames: f32) {
        let kb = KnockbackPlayback {
            dir,
            magnitude: level as f32 / KNOCKBACK_LEVEL_DIVISOR,
            animation_frames: animation_frames.max(0.0),
            run_time: 0.0,
            stand_up_started: false,
        };
        self.knockback = Some(kb);
        self.begin_completion_motion(
            DatId::from_str(KNOCKBACK_DOWN_CLIP),
            CompletionMotion {
                local_clips: &[],
                duration_frames: kb.total_frames(),
                max_loops: 1,
                transition_in: KNOCKBACK_DOWN_TRANSITION_IN,
                transition_out: KNOCKBACK_DOWN_TRANSITION_OUT,
            },
        );
    }

    /// Advances the knockback by `elapsed_frames` routine frames and returns
    /// this step's shove (KnockBackInstance updateEffect); starts the stand-up
    /// clip once the knock-down has run; `None` when no knockback is running.
    pub fn advance_knockback(&mut self, elapsed_frames: f32) -> Option<Vec2> {
        let mut kb = self.knockback?;
        kb.run_time += elapsed_frames;
        let shove = kb.dir * (elapsed_frames * kb.magnitude / KNOCKBACK_VELOCITY_DIVISOR);
        if !kb.stand_up_started && kb.run_time > kb.animation_frames {
            kb.stand_up_started = true;
            self.begin_completion_motion(
                DatId::from_str(KNOCKBACK_STAND_UP_CLIP),
                CompletionMotion {
                    local_clips: &[],
                    duration_frames: KNOCKBACK_STAND_UP_FRAMES,
                    max_loops: 1,
                    transition_in: KNOCKBACK_DOWN_TRANSITION_OUT,
                    transition_out: KNOCKBACK_DOWN_TRANSITION_IN,
                },
            );
        }
        self.knockback = (kb.run_time < kb.total_frames()).then_some(kb);
        Some(shove)
    }

    /// The run locks movement from its first frame to its last
    /// (KnockBackInstance `lockMovement(totalDuration / 2)` on the skeleton
    /// clock, the whole run on this one).
    pub fn knockback_active(&self) -> bool {
        self.knockback.is_some()
    }

    /// Cross-crate test seam: kuluu's movement-gate tests cannot reach the
    /// private engage field, so they drive it through here (Drawing or Engaged).
    pub fn set_engage_for_test(&mut self, drawing: bool) {
        self.engage = if drawing {
            EngageMachine::Drawing { remaining: 10.0 }
        } else {
            EngageMachine::Engaged
        };
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
        // N times and pins the end frame (SkeletonAnimator.kt applyLoopBounds). Retail holds the
        // pose for the whole authored loop count - the cast's mw2? hold, released when the
        // sequence ends - so the countdown must cover every loop or the cleared action leaves
        // the pinned end frame behind.
        let num_loops = (motion.max_loops != 0).then_some(motion.max_loops as u32);
        let loop_total = len * num_loops.unwrap_or(1) as f32;
        self.action = Some(ActionPlayback {
            clip_id,
            looping: num_loops.is_some(),
            remaining: loop_total.max(motion.duration_frames * 0.5).max(1.0),
            num_loops,
            transition_in: motion.transition_in.whole_frames(),
            transition_out: motion.transition_out.whole_frames(),
            cast_pose: false,
            settle: None,
        });
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum EngageMachine {
    NotEngaged,

    Drawing { remaining: f32 },
    Engaged,

    Sheathing { remaining: f32 },
}

// research/xim EffectRoutineInterpolatedEffects.kt KnockBackInstance: the knock-down clip
// plays for the stage's animationDuration, the stand-up for standUpTime = 8 frames after it;
// the velocity each tick is direction * elapsedFrames * magnitude / 8 with magnitude =
// wire level / 2; the whole run locks animation and movement. Frames are routine frames
// (scheduler_runtime::ROUTINE_FPS), the clock xim's interpolated effects tick on.
const KNOCKBACK_DOWN_CLIP: &str = "bf0?";
const KNOCKBACK_STAND_UP_CLIP: &str = "bf1?";
pub const KNOCKBACK_STAND_UP_FRAMES: f32 = 8.0;
const KNOCKBACK_LEVEL_DIVISOR: f32 = 2.0;
const KNOCKBACK_VELOCITY_DIVISOR: f32 = 8.0;
// KnockBackInstance's TransitionParams: 7.5 in / 3.5 out for the knock-down, mirrored for
// the stand-up, in whole frames; HalfFrames stores twice that.
const KNOCKBACK_DOWN_TRANSITION_IN: HalfFrames = HalfFrames::from_dat(15);
const KNOCKBACK_DOWN_TRANSITION_OUT: HalfFrames = HalfFrames::from_dat(7);

/// One knockback in flight on an actor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnockbackPlayback {
    /// Unit direction of the shove in FFXI x/y, away from the attacker.
    pub dir: Vec2,
    /// Wire level / KNOCKBACK_LEVEL_DIVISOR.
    pub magnitude: f32,
    /// The knock-down's length, the stage's animationDuration.
    pub animation_frames: f32,
    pub run_time: f32,
    pub stand_up_started: bool,
}

impl KnockbackPlayback {
    pub fn total_frames(&self) -> f32 {
        self.animation_frames + KNOCKBACK_STAND_UP_FRAMES
    }
}

/// The self actor's knockback as the walker sees it: the shove accumulated
/// since the walker last took it (FFXI x/y yalms), whether the run still locks
/// movement, and the attacker's position to face once when the shove begins
/// (KnockBackInstance `faceToward(source)`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct SelfKnockback {
    pub pending: Vec2,
    pub active: bool,
    pub face_toward: Option<Vec2>,
}

/// DAT transition fields are authored in half-frames: a stored value V plays as V/2 whole frames.
/// research/xim EffectRoutineInterpolatedEffects.kt divides the parsed u16 by 2 before handing it
/// to the skeleton domain, which ticks at half the routine clock (FRAME_RATE above).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HalfFrames(u16);

impl HalfFrames {
    pub const ZERO: Self = Self(0);

    /// A DAT-parsed transition field is already in this unit.
    pub const fn from_dat(v: u16) -> Self {
        Self(v)
    }

    /// A flinch transition derived from its stage's animationDuration in whole frames. XIM plays
    /// each side for duration/2 (EffectRoutineInterpolatedEffects.kt FlinchAnimationInstance), and
    /// V half-frames play as V/2, so the stored value is the total itself.
    pub fn from_flinch_total(total_whole_frames: f32) -> Self {
        Self((total_whole_frames.max(0.0)) as u16)
    }

    /// Whole-frame count this value plays as.
    pub const fn whole_frames(self) -> f32 {
        self.0 as f32 * 0.5
    }
}

pub struct CompletionMotion<'a> {
    pub local_clips: &'a [SkeletonAnimation],
    pub duration_frames: f32,
    pub max_loops: u16,
    pub transition_in: HalfFrames,
    pub transition_out: HalfFrames,
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

    /// The stage this playback hands off to when `clip_id` runs out, held until the action
    /// resolves (see [`settle_motion_clip`]). `None` releases the overlay to idle instead.
    settle: Option<DatId>,
}

impl ActionPlayback {
    /// Whether this playback outlives its own clip and so needs the action's resolution or
    /// interrupt to end it. A wind-up with a settled stage still pending counts: it is one
    /// frame away from the hold.
    fn held(&self) -> bool {
        self.looping || self.settle.is_some()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum RestPlayback {
    Inactive,

    Starting { kind: RestKind, remaining: f32 },

    Looping { kind: RestKind },

    Stopping { kind: RestKind, remaining: f32 },
}

impl LoadedActor {
    /// The model's Cib Info chunk when its primary DAT carries one (None for mounts and
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
    let (texture_names, cpu_images) = split_actor_textures(loaded.textures.clone(), q);
    let parts = prepare_actor_parts(loaded, texture_names, facing_dir, scale);
    let mesh_handles = add_part_meshes(&parts, meshes);
    let image_handles: Vec<Handle<Image>> =
        cpu_images.into_iter().map(|img| images.add(img)).collect();
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
        &image_handles,
        materials,
        material_cache,
        registry,
        &parts,
        actor_root,
        skin_slot,
        None,
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
    let assets = &loaded.action_assets;
    if assets.particle_defs.values().any(|d| d.auto_run)
        || assets.sound_defs.values().any(|d| d.auto_run)
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
pub struct ActorFadeMaterial(Handle<FfxiSkinnedMaterial>);

#[derive(Component)]
pub(crate) struct ActorMeshJointBounds {
    skin_slot: u32,
    joint_aabbs: Arc<[JointLocalAabb]>,
}

#[allow(clippy::too_many_arguments)]
fn build_actor_children(
    commands: &mut Commands,
    mesh_handles: &[Handle<Mesh>],
    image_handles: &[Handle<Image>],
    materials: &mut Assets<FfxiSkinnedMaterial>,
    material_cache: &mut FfxiSkinnedMaterialCache,
    registry: &mut FfxiSkinRegistry,
    parts: &PreparedParts,
    actor_root: Entity,
    skin_slot: u32,
    arrival: Option<bool>,
) -> Vec<u32> {
    let mut by_full: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(image_handles.len());
    let mut by_local: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(image_handles.len());
    let mut by_trimmed: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(image_handles.len());
    for (name, handle) in parts.texture_names.iter().zip(image_handles) {
        let trimmed = name.trim_end_matches(['\0', ' ']).to_string();
        if !trimmed.is_empty() {
            by_trimmed.entry(trimmed).or_insert(handle.clone());
        }
        let key = TextureKey::from_full(name);
        if key.local_name.is_empty() {
            continue;
        }
        by_full.entry(key.full_key()).or_insert(handle.clone());
        by_local.entry(key.local_name).or_insert(handle.clone());
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

        let opaque = material_cache.get_or_create(tex_handle.clone(), materials);
        let fading = arrival == Some(false);
        let mat = if fading {
            material_cache.get_for_phase(tex_handle, true, materials)
        } else {
            opaque.clone()
        };
        let instance_slot = registry.alloc_instance(FfxiInstance {
            flags: Vec4::new(has_texture, 0.0, 0.0, 0.0),
            tint: built.tint,
            skin_slot,
            reveal: if arrival == Some(true) { 0.0 } else { 1.0 },
            opacity: if fading { 0.0 } else { 1.0 },
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
        if fading {
            commands.entity(child).insert(ActorFadeMaterial(opaque));
        }
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

/// `pub` so the offline harnesses (and the rabbit tester's integration test) can build a render
/// actor straight from a LoadedActor without the mesh/material pipeline.
pub fn make_render_actor(
    loaded: &LoadedActor,
    skin_slot: u32,
    instance_slots: Vec<u32>,
    world_id: u32,
    facing_dir: f32,
    scale: f32,
) -> FfxiRenderActor {
    FfxiRenderActor {
        skeleton: loaded.skeleton.clone(),
        animations: loaded.all_animations(),
        battle_clips: loaded.all_battle_clips(),
        routines: loaded.all_routines(),
        action_assets: Arc::clone(&loaded.action_assets),
        rejected_clips: loaded.rejected_clips.clone(),
        rejected_routines: loaded.rejected_routines.clone(),
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
        body_armour_waist: loaded.cib.map(|c| c.body_armour_waist).unwrap_or(0),
        current_clip: None,
        registered_slots: [None; 8],
        pending_idle_registrations: [None; 8],
        rest_phase: RestPlayback::Inactive,
        death_phase: actor_state::DeathPhase::Unobserved,
        engage: EngageMachine::NotEngaged,
        knockback: None,
        action: None,
        event_idle: None,
        action_clips: Vec::new(),
        head_look: HeadLook::default(),
        locked_on: false,
        locked_torso_weight: 0.0,
        upper_body: crate::locked_torso::UpperBody::of(&loaded.skeleton),
        wire_animation: ffxi_proto::decode::animation::NONE,
        anim_modes: ActorAnimModes::default(),
        look_at_tasks: Vec::new(),
        look_at_visibility_word: 0,
        last_clip: None,
        last_frame: 0.0,
        world_pose: Vec::new(),
        pose_work: PoseScratch::default(),
        pose_scratch: BonePoseScratch::new(),
        weapon_handles: loaded.weapon_handles.clone(),
        point_light_selection: None,
    }
}

/// A LoadedActor with no model data behind it: the given skeleton and empty
/// clip/routine sets. The test constructors below share it.
fn empty_loaded_actor(skeleton: Skeleton, cib: Option<Cib>) -> LoadedActor {
    LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes: Vec::new(),
        effect_meshes: Vec::new(),
        textures: Vec::new(),
        animations: Arc::default(),
        battle_clips: Arc::default(),
        routines: Arc::default(),
        action_assets: Arc::default(),
        rejected_clips: Vec::new(),
        rejected_routines: Vec::new(),
        model_dat: String::new(),
        cib,
        weapon_handles: Vec::new(),
    }
}

/// The four animation-mode families a routine can switch (`ffxi_dat::scheduler::AnimModeSlot`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActorAnimModes {
    battle: u8,
    idle: u8,
    walking: u8,
    running: u8,
}

impl ActorAnimModes {
    /// Applies one `AnimationMode` stage to the slot it names.
    pub fn set(&mut self, slot: ffxi_dat::scheduler::AnimModeSlot, variant: u8) {
        match slot {
            ffxi_dat::scheduler::AnimModeSlot::Battle => self.battle = variant,
            ffxi_dat::scheduler::AnimModeSlot::Idle => self.idle = variant,
            ffxi_dat::scheduler::AnimModeSlot::Walking => self.walking = variant,
            ffxi_dat::scheduler::AnimModeSlot::Running => self.running = variant,
        }
    }
}

/// A posed actor with no model behind it, for tests that need the pose/skeleton pair a particle
/// attachment reads and nothing else.
#[cfg(test)]
pub(crate) fn render_actor_for_test(skeleton: Skeleton, world_pose: Vec<Mat4>) -> FfxiRenderActor {
    let loaded = empty_loaded_actor(skeleton, None);
    FfxiRenderActor {
        world_pose,
        ..make_render_actor(&loaded, 0, Vec::new(), 0, 0.0, 1.0)
    }
}

/// A no-model render actor with a chosen world id: cross-crate test seams
/// (kuluu's movement-gate tests) spawn one and read its engage state.
pub fn render_actor_stub(world_id: u32) -> FfxiRenderActor {
    let skeleton = Skeleton {
        id: DatId::from_str("test"),
        joints: Vec::new(),
        references: Vec::new(),
        bounding_boxes: Vec::new(),
        look_at_limits: Vec::new(),
    };
    make_render_actor(
        &empty_loaded_actor(skeleton, None),
        0,
        Vec::new(),
        world_id,
        0.0,
        1.0,
    )
}

/// A stub actor carrying the skeleton's clip set, the way load_pc fills
/// `animations` from the race skeleton DAT, for the cutscene cast tests.
#[cfg(test)]
pub(crate) fn render_actor_with_skeleton_clips(
    world_id: u32,
    clips: Vec<SkeletonAnimation>,
) -> FfxiRenderActor {
    let mut actor = render_actor_stub(world_id);
    actor.animations = Arc::new(clips);
    actor
}

/// A render actor with no model behind it, carrying an explicit Cib Info movement byte, for the
/// remote-grounding test that gates on MovementType and needs nothing else.
#[cfg(test)]
pub(crate) fn render_actor_with_movement_for_test(
    skeleton: Skeleton,
    world_pose: Vec<Mat4>,
    movement_type: MovementType,
) -> FfxiRenderActor {
    let cib = Cib {
        movement_type,
        ..Cib::parse(*b"cib0", &[0u8; ffxi_dat::cib::CIB_LEN]).unwrap()
    };
    let loaded = empty_loaded_actor(skeleton, Some(cib));
    FfxiRenderActor {
        world_pose,
        ..make_render_actor(&loaded, 0, Vec::new(), 0, 0.0, 1.0)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_live_actor(
    commands: &mut Commands,
    mesh_handles: &[Handle<Mesh>],
    image_handles: &[Handle<Image>],
    materials: &mut Assets<FfxiSkinnedMaterial>,
    material_cache: &mut FfxiSkinnedMaterialCache,
    registry: &mut FfxiSkinRegistry,
    prepared: &PreparedActor,
    wire_entity: Entity,
    world_id: u32,
    scale: f32,
    enhanced_arrival: bool,
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
        image_handles,
        materials,
        material_cache,
        registry,
        &prepared.parts,
        actor_root,
        skin_slot,
        Some(enhanced_arrival),
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

/// Rebuilds an actor texture through the zone path's mip/anisotropic builder. Alpha is left
/// exactly as the decoder produced it (the zone alpha remap does not apply to actors), so only
/// filtering changes, following the GUI Texture Filtering setting like the zone/MMB paths.
/// Nothing reads actor texels back on the CPU, and a Texture Filtering change reloads the look
/// through `ActorPrepKey` rather than patching the asset, so the main-world copy is dropped at
/// upload (`RenderAssetUsages::RENDER_WORLD`).
fn decoded_texture_to_image(t: DecodedTexture, q: crate::zone_texture::TextureQuality) -> Image {
    let cutout = crate::zone_texture::has_cutout_alpha(&t);
    let mut img = crate::zone_texture::image_with_mips(t.rgba, t.width, t.height, q, cutout);
    img.asset_usage = RenderAssetUsages::RENDER_WORLD;
    img
}

/// Pose one actor outside the live snapshot path, for the offline render
/// harnesses. `mount` is what `tick_live_ffxi_actors` derives from the mount
/// actor's own pose; passing it here keeps the seat maths in one place.
pub fn advance_actor_pose_standalone(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    mount: Option<MountAttach>,
) {
    advance_actor_pose(
        actor,
        elapsed_frames,
        crate::look_at_gates::LookState::IdleNoTarget,
        None,
        mount,
        false,
        None,
    );
}

/// The same standalone advance with the caller's routine-lock state instead of
/// the fixed `false`: the flag the full pose system passes from
/// scheduler_runtime's `is_locked_now`.
#[cfg(test)]
pub(crate) fn advance_actor_pose_standalone_locked(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    animation_locked: bool,
) {
    advance_actor_pose(
        actor,
        elapsed_frames,
        crate::look_at_gates::LookState::IdleNoTarget,
        None,
        None,
        animation_locked,
        None,
    );
}
pub fn tick_ffxi_render_actors(
    time: Res<Time>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut q_actors: Query<&mut FfxiRenderActor>,
) {
    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    q_actors.par_iter_mut().for_each(|mut actor| {
        advance_actor_pose(
            &mut actor,
            elapsed_frames,
            crate::look_at_gates::LookState::IdleNoTarget,
            None,
            None,
            false,
            None,
        );
    });
    for actor in &q_actors {
        registry.set_skin_joints(actor.skin_slot, &actor.world_pose);
    }
}

/// Direct resolution of a requested clip id against the available sets: overlay first, then
/// primary, deduped by exact chunk id. No fallback here; an empty result is information (the
/// model ships no usable chunk for this parameterized id), and the caller decides what to do
/// with it.
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

/// Advances the rest state machine one frame. Only the middle (Loop) phase loops; In/Out are
/// one-shots that hold their last frame (looping them replays the kneel from frame 0 as the
/// character finishes standing up).
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

/// Why a requested routine yielded no motion clip. Each case gets its own CLIP_WARN reason so
/// the miss names itself instead of degrading to locomotion silently.
#[derive(Debug)]
enum RoutineMiss {
    /// No scheduler chunk with this name in the model's sets: retail is a no-op here too.
    NotFound,
    /// A chunk with this name was seen in the DAT walk but its parse was rejected; the parse
    /// reason itself lives on `LoadedActor.rejected_routines`.
    SeqLoadError,
    /// The routine parsed fine but carries no Motion stage.
    NoMotionStage,
}

/// `last_stage` picks which Motion stage the caller wants. Every `fsh<n>` routine carries two
/// (a wind-up and the pose it settles into; `fsh0` = `fh0?` cast then `fh1?` wait, `fsh1` =
/// `fh8?` set-the-hook then `fh2?` fight). A phase the client holds for an indefinite time has to
/// loop the settled stage; looping the wind-up instead replays the cast over and over.
fn routine_motion_lookup(
    routines: &HashMap<DatId, Scheduler>,
    rejected_routines: &[ffxi_dat::resource_dir::RejectedRoutine],
    routine: DatId,
    last_stage: bool,
) -> Result<Option<DatId>, RoutineMiss> {
    let Some(sched) = routines.get(&routine) else {
        return Err(if rejected_routines.iter().any(|r| r.name == routine.0) {
            RoutineMiss::SeqLoadError
        } else {
            RoutineMiss::NotFound
        });
    };
    let motion = if last_stage {
        sched
            .stages
            .iter()
            .rev()
            .find(|t| t.stage.kind == StageKind::Motion)
    } else {
        sched
            .stages
            .iter()
            .find(|t| t.stage.kind == StageKind::Motion)
    };
    match motion {
        Some(t) => Ok(Some(DatId::from_name(&t.stage.id))),
        None => Err(RoutineMiss::NoMotionStage),
    }
}

fn routine_motion_clip(
    routines: &HashMap<DatId, Scheduler>,
    rejected_routines: &[ffxi_dat::resource_dir::RejectedRoutine],
    routine: DatId,
) -> Option<DatId> {
    routine_motion_lookup(routines, rejected_routines, routine, false)
        .ok()
        .flatten()
}

/// CLIP_WARN for a requested routine that yielded no motion clip; the routine name stands in
/// for the clip field because there is no motion clip to name. As with `clip_miss`, the reason
/// stays in the dedupe key so two distinct misses on one pair both print.
fn routine_motion_miss(id: u32, name: &str, model: &str, routine: DatId, miss: &RoutineMiss) {
    let reason = match miss {
        RoutineMiss::NotFound => "routine_not_found",
        RoutineMiss::SeqLoadError => "routine_seq_load_error",
        RoutineMiss::NoMotionStage => "routine_no_motion_stage",
    };
    clip_warn_once(id, name, model, &routine, reason);
}

/// The stage a once-through start routine settles into after its wind-up, or `None` when the
/// routine carries a single Motion stage and so has nothing to hold. `cait` is authored this way
/// (`dat-routine-stages 7072 ca`: `mi0?` dur=28 half-frames at frame 0, then `mi1?` dur=99) to
/// cover an item's activation window — `item_usable.activation`, 2s for the Hatchling Shield
/// (vendor/server/src/map/ai/states/item_state.cpp CItemState::Update) — which the wind-up alone
/// is a third of.
fn settle_motion_clip(
    routines: &HashMap<DatId, Scheduler>,
    rejected_routines: &[ffxi_dat::resource_dir::RejectedRoutine],
    routine: DatId,
    wind_up: DatId,
) -> Option<DatId> {
    routine_motion_lookup(routines, rejected_routines, routine, true)
        .ok()
        .flatten()
        .filter(|last| *last != wind_up)
}

/// The `ded?` collapse clip and the routine-authored frames retail plays it for
/// before swapping to the held `cor?` corpse pose. Both are Motion stages of the
/// `dead` routine (`dat-routine-stages 7072 dead`: `ded?` dur=116 half-frames at
/// frame 0, `cor?` at frame 116), and the duration is per race - 68..156
/// half-frames across the seven PC skeletons - so it is read from the routine, not assumed.
fn death_collapse_clip(routines: &HashMap<DatId, Scheduler>) -> Option<(DatId, f32)> {
    let sched = routines.get(&actor_state::death_routine_id())?;
    let mut motions = sched
        .stages
        .iter()
        .filter(|t| t.stage.kind == StageKind::Motion);
    let collapse = motions.next()?;
    // SAFETY: a routine with a single Motion stage has no corpse pose to fall through to, so
    // the `?` turns it into no-collapse (None) rather than holding `ded?` forever.
    motions.next()?;
    Some((
        DatId::from_name(&collapse.stage.id),
        half_frames(collapse.stage.duration_frames),
    ))
}

pub(crate) use ffxi_vocab::magic::CATEGORY_MAGIC_START as MAGIC_START_CATEGORY;

// The actor's shot motion on a ranged finish, one-shot for every weapon type.
// research/xim/src/jsMain/kotlin/xim/poc/Actor.kt onRangedAttack.
const RANGED_FINISH_ROUTINE: &str = "shlg";

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
            // A valid "ca??" start keeps its category's looping semantics
            // (vendor/server/src/map/enums/four_cc.h): the generic `cast`/`calg` loops its single
            // Motion stage until resolution, while `cate`/`cait` are authored as a wind-up plus a
            // settled stage and hold the second one instead (see `settle_motion_clip`).
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

        ffxi_proto::melee::CATEGORY_RANGED_FINISH => {
            (DatId::from_str(RANGED_FINISH_ROUTINE), false)
        }

        _ => return None,
    })
}

/// The goals under which the reactor moves the player itself, so the self
/// pose reads the wire motion rather than the keys. The same set
/// `snapshot_drives_movement` (kuluu view_native/input.rs) gates the movement
/// dispatch on: an engage goal never moves the player (`Reactor::handle_command`
/// forwards `Move` untouched while engaged), so its pose stays key-driven.
fn self_pose_follows_reactor(goal: Option<&kuluu_snapshot::ReactorGoal>) -> bool {
    matches!(
        goal,
        Some(
            kuluu_snapshot::ReactorGoal::Following { .. }
                | kuluu_snapshot::ReactorGoal::Pathing { .. }
                | kuluu_snapshot::ReactorGoal::Banking { .. }
        )
    )
}

/// Re-issue the idle-family requests a slot declined, until each lands. The handover gate is
/// per-animator, so an overlay that has not finished its pass refuses the low-priority request and only
/// becomes eligible once it has pinned its last pose — one frame later than the selection that asked.
fn retry_idle_registrations(
    coordinator: &mut SkeletonAnimationCoordinator,
    animations: &[SkeletonAnimation],
    battle_clips: &[SkeletonAnimation],
    registered_slots: &mut [Option<(DatId, bool)>; 8],
    pending: &mut [Option<(DatId, bool)>; 8],
) {
    if std::env::var("KULUU_NO_RETRY").is_ok() {
        return;
    }
    for (slot, request) in pending.iter_mut().enumerate() {
        let Some(book) = *request else {
            continue;
        };
        let live = coordinator.animations[slot]
            .as_ref()
            .and_then(|a| a.current_animation.as_ref())
            .map(|c| c.animation.id);
        if live == Some(book.0) {
            registered_slots[slot] = Some(book);
            *request = None;
            continue;
        }
        let pool: &[SkeletonAnimation] = if book.1 { battle_clips } else { animations };
        let Some(clip) = pool.iter().find(|c| c.id == book.0).cloned() else {
            *request = None;
            continue;
        };
        if coordinator.register_idle_animation(clip, true) {
            registered_slots[slot] = Some(book);
            *request = None;
        }
    }
}

/// The draw/sheathe window is the length of the `in 0`/`out0` routine's
/// motion clip as it plays: the battle set's copy, or the base set's only when
/// the battle set has none (`select_pose_clips_layered` overlays the battle
/// set first). Not the max of the two: `rest_clip_len_frames` matches the `?`
/// wild across every variant in a set, and the base PC set holds draw clips
/// for weapons not in hand.
fn advance_engage(
    machine: &mut EngageMachine,
    want_engaged: bool,
    routines: &HashMap<DatId, Scheduler>,
    rejected_routines: &[ffxi_dat::resource_dir::RejectedRoutine],
    battle_clips: &[SkeletonAnimation],
    animations: &[SkeletonAnimation],
    elapsed_frames: f32,
) -> actor_state::EngageAnimationState {
    use actor_state::EngageAnimationState as S;

    let transition_len = |routine: &str| -> f32 {
        routine_motion_clip(routines, rejected_routines, DatId::from_str(routine))
            .map(|clip| {
                let battle = rest_clip_len_frames(battle_clips, clip);
                if battle > 0.0 {
                    battle
                } else {
                    rest_clip_len_frames(animations, clip)
                }
            })
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

/// The precedence tier that owns a frame's pose selection, walked in declaration order (death
/// collapse > action > engage overlay > fishing > special > rest > locomotion). A tier claims the
/// pose only when its clip resolves to at least one usable chunk in this model's sets; a
/// requested clip the model does not ship warns once and falls through to the next tier instead
/// of pinning current_clip. That is what keeps an entity whose named routine the model does not
/// ship out of the special override: its clip fails to resolve, so selection drops to locomotion
/// and the walk/idle clip loops normally rather than registering as a one-shot that holds its
/// end frame. No per-mob or pool gating decides this; only what the DAT resolves does. CLIP_WARN
/// names the tier so a miss on an override (special/fishing asking for a clip the model does not
/// ship) is distinguishable from a miss on the base tiers.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PoseTier {
    Death,
    Action,
    EngageOverlay,
    Fishing,
    Special,
    Rest,
    Locomotion,
}

/// Clears the action/engage state and re-poses with `animation_locked = false` (the action was
/// just cleared, so no lock can be in effect). The death phase is reset after the re-pose, whose
/// default (alive) inputs would otherwise read as having watched this actor alive: retail only
/// plays `ded?` for a death it saw, so a KO'd zone-in resumes on the held corpse frame.
fn reset_actor_pose_state(actor: &mut FfxiRenderActor, elapsed_frames: f32, name: Option<&str>) {
    actor.inputs = ActorAnimInputs::default();
    actor.rest_phase = RestPlayback::Inactive;

    actor.action = None;
    actor.engage = EngageMachine::NotEngaged;
    actor.knockback = None;
    actor.look_at_tasks.clear();
    actor.wire_animation = ffxi_proto::decode::animation::NONE;
    actor.coordinator.clear();
    // A carried record belongs to the clip that wrote it, so a reset drops them with the layers: the
    // next pose pass starts from the bind pose again.
    actor.pose_scratch.clear();
    actor.current_clip = None;
    actor.registered_slots = [None; 8];
    actor.pending_idle_registrations = [None; 8];
    advance_actor_pose(
        actor,
        elapsed_frames,
        crate::look_at_gates::LookState::IdleNoTarget,
        None,
        None,
        false,
        name,
    );
    actor.death_phase = actor_state::DeathPhase::Unobserved;
}

/// Runs inside the parallel per-actor pass: it touches only the actor's own fields, leaving the
/// pose in `world_pose` for the serial registry copy.
fn advance_actor_pose(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    // Whether this frame aims at all — decided from the current state, never from what the last frame
    // was doing (`look_at_gates::LookState`).
    look_state: crate::look_at_gates::LookState,
    // One frame of look-at (`LookAtInput`); its rotation is the only orientation that enters
    // `look_point_actor_local`, and no pitch or slope does.
    look: Option<LookAtInput>,
    mount: Option<MountAttach>,
    animation_locked: bool,
    name: Option<&str>,
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
        registered_slots,
        pending_idle_registrations,
        rest_phase,
        death_phase,
        engage,
        action,
        event_idle,
        action_clips,
        head_look,
        locked_on,
        locked_torso_weight,
        upper_body,
        wire_animation,
        last_clip,
        last_frame,
        world_pose,
        pose_work,
        pose_scratch,
        weapon_handles,
        rejected_clips,
        rejected_routines,
        model_dat,
        ..
    } = actor;
    let animations: &[SkeletonAnimation] = animations;
    let battle_clips: &[SkeletonAnimation] = battle_clips;

    let action_pre = action.is_some();
    let action_id = match action.as_mut() {
        Some(act) => {
            act.remaining -= elapsed_frames;
            // While an AnimationLock is held (StageKind::AnimationLock, ffxi-dat/src/scheduler.rs),
            // the routine's Motion stage owns the clip: keep it selected past its own length
            // instead of releasing to idle. One-shots pin their end frame in the coordinator
            // (num_loops = 1), so this is what holds a buried/emerged pose until the lock lapses.
            if act.remaining <= 0.0 && !animation_locked {
                match act.settle {
                    Some(settled) => {
                        act.clip_id = settled;
                        act.settle = None;
                        act.looping = true;
                        act.num_loops = None;
                        act.remaining = CAST_TIMEOUT_FRAMES;
                        Some(settled)
                    }
                    None => {
                        *action = None;
                        action_clips.clear();
                        None
                    }
                }
            } else {
                Some(act.clip_id)
            }
        }
        None => None,
    };
    // The frame the action's clip loses the pose: its slot must hand over to
    // the idle selection at once (below) instead of waiting for the clip to
    // finish looping, which a hold loop never does on its own.
    let action_just_ended = action_pre && action.is_none();

    let engage_overlay = match *engage {
        EngageMachine::Drawing { .. } | EngageMachine::Sheathing { .. } => {
            let routine = if matches!(*engage, EngageMachine::Drawing { .. }) {
                DatId::from_str("in 0")
            } else {
                DatId::from_str("out0")
            };
            match routine_motion_lookup(routines, rejected_routines, routine, false) {
                Ok(clip) => clip,
                Err(miss) => {
                    routine_motion_miss(
                        actor.world_id,
                        name.unwrap_or("-"),
                        model_dat,
                        routine,
                        &miss,
                    );
                    None
                }
            }
        }
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
            let motion = match if fc.looping {
                routine_motion_lookup(routines, rejected_routines, fc.id, true)
            } else {
                routine_motion_lookup(routines, rejected_routines, fc.id, false)
            } {
                Ok(clip) => clip,
                Err(miss) => {
                    routine_motion_miss(
                        actor.world_id,
                        name.unwrap_or("-"),
                        model_dat,
                        fc.id,
                        &miss,
                    );
                    None
                }
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
    // interpretation: what the sub value does on this model is defined by its DAT alone (the
    // wire-state machine is ffxi-actor/src/actor_state.rs next_special_pose).
    //
    // The pose is held by whichever retail mechanism is active: the wire slot (a sub change on a
    // live actor - the dig's buried pose, held until the sub clears or the resurface) or the
    // routine's own AnimationLock (the resurface's 'init' on retail's slot-less fresh actor).
    // Once both lapse the pose falls to idle even if the server keeps the sub set: the wire
    // carries no stop-animation signal, so nothing here waits for one.
    let special_held = inputs.special.slot_held || animation_locked;
    let special_clip_id = inputs
        .special
        .active_routine
        .filter(|_| special_held)
        .and_then(|routine_name| {
            let routine = DatId::from_name(&routine_name);
            match routine_motion_lookup(routines, rejected_routines, routine, false) {
                Ok(clip) => clip,
                Err(miss) => {
                    routine_motion_miss(
                        actor.world_id,
                        name.unwrap_or("-"),
                        model_dat,
                        routine,
                        &miss,
                    );
                    None
                }
            }
        });

    // Retail's `dead` routine outranks locomotion and any in-flight action, so the collapse
    // heads the selection chain; when its timer expires the held `cor?` takes over through the
    // locomotion tier (ffxi-actor/src/actor_state.rs next_death_phase). A missing collapse clip
    // would stall the corpse in Collapsing behind a pose that does not draw, so the clip is
    // filtered to what this model ships: a dead actor without `ded?` settles straight to the
    // held `cor?`.
    let dead = actor_state::corpse_pose_selected(inputs);
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

    let mut one_shot_rest = false;
    let try_tier = |id: DatId, is_idle: bool, tier: PoseTier| {
        if usable(&resolve(id)) {
            Some((id, is_idle, tier))
        } else {
            clip_miss(
                actor.world_id,
                name.unwrap_or("-"),
                model_dat,
                &id,
                rejected_clips,
                tier,
            );
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
            // SAFETY (ffxi-actor/src/actor_state.rs RestPhase): the rest state machine advances
            // only once selection reaches the Rest tier; a higher-priority override that
            // resolved above leaves it untouched.
            let id = advance_rest_phase(rest_phase, inputs.rest, animations, elapsed_frames)?;
            let looping = matches!(rest_phase, RestPlayback::Looping { .. });
            let chosen = try_tier(id, looping, PoseTier::Rest)?;
            one_shot_rest = !looping;
            Some(chosen)
        })
        .or_else(|| {
            let s = actor_state::selected_animation(inputs);
            s.idle
                .then_some(*event_idle)
                .flatten()
                .and_then(|id| try_tier(id, true, PoseTier::Locomotion))
                .or_else(|| try_tier(s.id, s.idle, PoseTier::Locomotion))
        });

    // Terminal fallback: even the lowest tier resolved to nothing, so fall back to the idle family
    // (ffxi-actor/src/actor_state.rs idle_animation_id; retail's behavior). If that too is empty
    // there is genuinely no usable clip in this model and current_clip stays untouched; that is
    // the frozen-mob signature CLIP_WARN reports.
    let (selected_id, is_idle, selected_tier) = match chosen {
        Some(c) => c,
        None => {
            clip_warn_once(
                actor.world_id,
                name.unwrap_or("-"),
                model_dat,
                &DatId::from_str("idl?"),
                "not_found",
            );
            (DatId::from_str("idl?"), true, PoseTier::Locomotion)
        }
    };

    // A chosen tier resolved to a usable chunk above, so this is non-empty unless we fell
    // through every tier and the idle family itself (ffxi-actor/src/actor_state.rs
    // idle_animation_id) is missing from the model.
    let matches: Vec<&SkeletonAnimation> = resolve(selected_id);
    if matches.is_empty() {
        clip_warn_once(
            actor.world_id,
            name.unwrap_or("-"),
            model_dat,
            &selected_id,
            "no_match_kept_previous",
        );
    }

    // Gated special-pose diagnostics (ffxi-actor/src/actor_state.rs SpecialPose): log every
    // pose-selection change that touches the active routine's clip or happens while a special
    // state is up. Catches a silent fall-through (the model ships no usable chunk for the named
    // routine) and a higher-priority override releasing the one-shot before INVISIBLE arrives.
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

    // The coordinator cursor resets only when the selected clip actually changes (wlk<->run,
    // moving<->idle, or a tier/clip-set change such as casual->battle on engage). A new POS update
    // within the same gait is not a clip change: re-registering here would restart the walk clip
    // from frame 0 on every packet (the stop-and-go stutter of B). The chase model keeps `moving`
    // up across a late update via its hold-until grace window (combat_stance.rs PredictSample), so
    // an unchanged gait does not reach this branch. There is no per-frame or per-update
    // re-registration path for an unchanged locomotion clip: coordinator.update below advances
    // the cursor monotonically instead.
    if !matches.is_empty() && *current_clip != Some((selected_id, use_battle)) {
        if let Some(resolved) = matches.iter().find(|a| is_usable_clip(a)) {
            clip_ok(actor.world_id, &selected_id, resolved, actor.movement_type);
        }
        let previous = *current_clip;
        if anim_traced(actor.world_id) {
            tracing::info!(
                target: "clip",
                "ANIM_SEL id={:#x} name={} {} -> {} tier={:?} battle={} engage={:?} clips=[{}]",
                actor.world_id,
                name.unwrap_or("-"),
                previous.map_or("-".to_string(), |(id, b)| format!("{}/b{b}", id.as_str())),
                selected_id.as_str(),
                selected_tier,
                use_battle,
                engage,
                matches
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        *current_clip = Some((selected_id, use_battle));
        // Any outstanding low-priority request belongs to the selection that made it.
        for pending in pending_idle_registrations.iter_mut() {
            *pending = None;
        }

        let mut new_mask = 0u8;
        for clip in &matches {
            let slot = (clip.id.final_digit().unwrap_or(0) as usize).min(7);
            new_mask |= 1 << slot;
        }

        // Slots the new selection drops are cleared outright; a slot that keeps the same clip
        // (id and set) is skipped in the register loops below so the clip keeps its frame instead
        // of restarting. This stands in for retail's own anti-restart rule: xim re-requests the
        // movement clips on every update frame at low priority (research/xim ActorModel.kt
        // transitionToMoving -> setSkeletonAnimation), and SkeletonAnimator.setNextAnimation
        // returns untouched when the request carries the same animation object as a low-priority
        // current. kuluu's selection pass clones its clips, so object identity cannot carry that
        // rule — the (id, set) tuple per slot is the equivalent: unchanged slot keeps its playhead.
        for (slot, reg) in registered_slots.iter_mut().enumerate() {
            if new_mask & (1 << slot) == 0 {
                coordinator.clear_slot(slot);
                *reg = None;
            }
        }

        if is_idle {
            for &clip in &matches {
                let slot = (clip.id.final_digit().unwrap_or(0) as usize).min(7);
                let from_battle = battle_clips.iter().any(|b| std::ptr::eq(b, clip));
                if registered_slots[slot] == Some((clip.id, from_battle)) {
                    continue;
                }
                let accepted = if action_just_ended {
                    coordinator.register_idle_animation_eager(clip.clone())
                } else {
                    coordinator.register_idle_animation(clip.clone(), true)
                };
                // A declined handover is not a registration: the slot still holds whoever refused it,
                // so record the request as outstanding rather than satisfied.
                if accepted {
                    registered_slots[slot] = Some((clip.id, from_battle));
                    pending_idle_registrations[slot] = None;
                } else {
                    pending_idle_registrations[slot] = Some((clip.id, from_battle));
                }
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
            // Special-pose and death-collapse clips are one-shots (the routine's motion clip
            // holds its end frame until the wire state settles; ffxi-actor/src/actor_state.rs).
            // Keyed on the winning tier so an entity whose named routine does not resolve loops
            // its locomotion clip instead of pinning it.
            // The engage transition clips are one-shots too. Traced live at the default cap
            // (KULUU_ANIM_LOG): `in 0`'s motion played to its last frame and wrapped back to
            // frame 0 on the closing step of the window (`| ina0:35.50 | -> | ina0:0.00 |`), so
            // the pull-out restarted itself under the crossfade into battle idle — and identically
            // at the sheathe edge, which is what reads as "it plays those frames again".
            let loop_params = LoopParams {
                loop_duration: None,
                num_loops: action.and_then(|a| a.num_loops).or((one_shot_fishing
                    || one_shot_rest
                    || matches!(selected_tier, PoseTier::Death)
                    || matches!(selected_tier, PoseTier::Special)
                    || matches!(selected_tier, PoseTier::EngageOverlay))
                .then_some(1)),
                low_priority: false,
            };
            for &clip in &matches {
                let slot = (clip.id.final_digit().unwrap_or(0) as usize).min(7);
                let from_battle = battle_clips.iter().any(|b| std::ptr::eq(b, clip));
                if registered_slots[slot] == Some((clip.id, from_battle)) {
                    continue;
                }
                coordinator
                    .register_animation(clip.clone(), loop_params, Some(tp.clone()), |_| true);
                registered_slots[slot] = Some((clip.id, from_battle));
            }
        }
    }

    retry_idle_registrations(
        coordinator,
        animations,
        battle_clips,
        registered_slots,
        pending_idle_registrations,
    );

    if anim_traced(actor.world_id) {
        // `registered_slots` decides whether a slot gets re-registered, so a layer in the coordinator
        // that the bookkeeping does not name can never be replaced: every later frame takes it as
        // already present. Saying so here beats waiting for the pose to look wrong.
        for (slot, animator) in coordinator.animations.iter().enumerate() {
            let live = animator
                .as_ref()
                .and_then(|a| a.current_animation.as_ref())
                .map(|c| c.animation.id);
            if pending_idle_registrations[slot].is_some() {
                continue;
            }
            let book = registered_slots[slot];
            let differs = match (live, book) {
                (None, None) => false,
                (Some(live), Some(book)) => live != book.0,
                _ => true,
            };
            if differs {
                tracing::warn!(
                    target: "clip",
                    "ANIM_STALE id={:#x} slot {slot}: coordinator={} book={}",
                    actor.world_id,
                    live.map_or("-".to_string(), |id| id.as_str().to_string()),
                    book.map_or("-".to_string(), |(id, battle)| format!("{}/b{battle}", id.as_str())),
                );
            }
        }
    }

    *last_clip = matches
        .iter()
        .max_by_key(|a| a.key_frame_sets.len())
        .map(|a| a.id);

    // Retail's AnimationSpeed (SpeedBase * 0.1, ffxi-actor/src/actor_state.rs playback_rate)
    // scales walk/run clip playback relative to the authored rate; idle and override tiers play
    // at their authored pace, so only a winning non-idle locomotion tier takes the scale.
    let frame_step = if matches!(selected_tier, PoseTier::Locomotion) && !is_idle {
        elapsed_frames * inputs.playback_rate
    } else {
        elapsed_frames
    };
    let trace = anim_traced(actor.world_id);
    let before = trace.then(|| anim_playheads(coordinator));

    coordinator.update(frame_step);

    if trace {
        tracing::info!(
            target: "clip",
            "ANIM_TRACE id={:#x} sel={} battle={} tier={:?} engage={:?} step={frame_step:.2} |{}| -> |{}|",
            actor.world_id,
            selected_id.as_str(),
            use_battle,
            selected_tier,
            engage,
            before.as_deref().unwrap_or(""),
            anim_playheads(coordinator),
        );
    }

    *last_frame = coordinator
        .animations
        .iter()
        .flatten()
        .filter_map(|a| a.current_animation.as_ref().map(|c| c.current_frame))
        .next_back()
        .unwrap_or(0.0);

    // Gated hold probe: while a special state is up (ffxi-actor/src/actor_state.rs SpecialPose),
    // sample the pinned frame every 30 ticks so a released one-shot shows up as last_frame
    // drifting back toward 0. `done` reports whether the routine's motion clip has played to its
    // end frame; pinning (num_loops = 1) keeps it on that frame until the wire state settles.
    // Retail does not hide a model on clip completion, so any drift here is a bug, not something
    // a visibility flag papers over.
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

    // Sample into retail's per-bone records before composing: every record is rebuilt from the live
    // layers this frame and a bone no layer keys falls back to its bind data, exactly as xim composes
    // (research/xim resource/SkeletonInstance.kt updateCurrentJointTransform). A held weapon therefore
    // does not need a carried record - it needs the handle re-parent below.
    coordinator.sample_pose(skeleton.joints.len(), pose_scratch);

    // Retail re-parents each equipped weapon's handle joint onto the bone its hand standard position
    // names, and only while the actor is display-engaged (research/xim resource/SkeletonInstance.kt
    // computeJointParentOverrides). The override drops that joint's own authored transform: measured on
    // Hume M, a strafe clip puts handle joints 3/4/9 between 150 and 178 degrees away from the battle
    // stance, so a held mesh left riding its chain swings across the body on every direction change.
    let mut handle_overrides: Vec<(usize, usize)> = Vec::new();
    if use_battle {
        handle_overrides.extend(weapon_handles.iter().copied());
    }

    let side_step = matches!(selected_tier, PoseTier::Locomotion)
        && inputs.moving
        && !inputs.walking
        && matches!(
            actor_state::movement_direction(inputs.forward_vel, inputs.strafe_vel),
            actor_state::Direction::Left | actor_state::Direction::Right
        );
    let torso_step = elapsed_frames / LOCOMOTION_XFADE_IN;
    *locked_torso_weight = if *locked_on && side_step {
        (*locked_torso_weight + torso_step).min(1.0)
    } else {
        (*locked_torso_weight - torso_step).max(0.0)
    };
    let steered_spine = match upper_body.as_ref() {
        Some(upper) if *locked_on || *locked_torso_weight > 0.0 => upper.steered_spine(coordinator),
        _ => Vec::new(),
    };

    pose_world_mounted_into(
        world_pose,
        pose_work,
        skeleton,
        |joint| {
            steered_spine
                .iter()
                .find(|(steered, _)| *steered == joint)
                .map(|(_, record)| *record)
                .or_else(|| pose_scratch.get(joint))
        },
        RootTransform {
            facing_dir: *facing_dir,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::splat(*scale),
        },
        &handle_overrides,
        mount,
    );

    // Retail removes the actor's yaw from a posed point and nothing else (`FFXiMain.dll retail-2026-09` RVA
    // 0xD5CDA..0xD5D25 — [`look_point_actor_local`]). kuluu composes `world_pose` with the entity transform, so that
    // same rotation is what takes the yaw out here; taking it from anywhere else leaves heading inside the look vector
    // and trades side for elevation. `FfxiRenderActor::facing_dir` stays 0 on purpose — baking a second heading onto
    // the pose would rotate the body twice.
    let target_pose = look.map(|input| {
        look_point_actor_local(
            input.pose_rotation,
            *scale,
            input.actor_world,
            input.target_attach_world,
        )
    });
    // The state decides whether this frame aims at all; retail's release test is positional and only
    // applies once it does.
    let aiming = look_state == crate::look_at_gates::LookState::Aiming
        && target_pose.is_some_and(|p| !look_at_release(p));
    head_look.advance(aiming, target_pose, elapsed_frames);
    let bend_records = look_at_gates::look_at_bend_records(*wire_animation);
    // The chased point is already in the space `world_pose` lives in (yaw removed once at the boundary above), so it
    // reaches the bend untouched and every authored limit opens along this actor's own nose.
    // Locked and side-stepping, the bend measures from where the chest really faces, and it runs even under a status
    // that holds the look-at off: the walker keeps the target dead ahead of the root, so with nothing else aiming the
    // point is that target at neck height.
    let bend_forward = crate::locked_torso::bend_reference(
        POSE_FORWARD,
        upper_body
            .as_ref()
            .and_then(|upper| upper.chest_facing(world_pose)),
        *locked_torso_weight,
    );
    let bend_point = if head_look.weight > 0.0 {
        Some(head_look.chased_pose)
    } else {
        ffxi_actor::look_bend::attach_frame(
            world_pose,
            skeleton,
            ffxi_dat::skel::standard_position::NECK,
        )
        .map(|neck| neck.origin + POSE_FORWARD * HEAD_LOOK_NEUTRAL_AHEAD)
    };
    if let Some(bend_point) = bend_point {
        apply_look_bends(
            world_pose,
            skeleton,
            &handle_overrides,
            bend_point,
            bend_forward,
            head_look.weight.max(*locked_torso_weight),
            bend_records,
            look_at_gates::look_at_anchor_y_drop(*wire_animation),
        );
    }

    if let Some(detail) = bone_log_detail() {
        bone_trace_frame(
            detail,
            actor.world_id,
            selected_id,
            registered_slots,
            coordinator,
            skeleton,
            world_pose,
        );
    }
}

/// `KULUU_BONE_LOG` — a play-test probe for "which bone ended up where": one line per posed frame
/// listing every joint's position in the actor-local pose space (the space `world_pose` lives in, so
/// facing is already out of it), plus axes and `keyed_by` provenance for the joints named in the env
/// value (`KULUU_BONE_LOG=87,126,127`), or for every joint with `KULUU_BONE_LOG=all`. A joint no
/// active clip keys appears in no `keyed_by` list.
fn bone_log_detail() -> Option<&'static [usize]> {
    static SPEC: std::sync::OnceLock<Option<Vec<usize>>> = std::sync::OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        std::env::var("KULUU_BONE_LOG").ok().map(|v| {
            if v.trim() == "all" {
                return vec![BONE_LOG_ALL];
            }
            v.split([',', ' '])
                .filter_map(|s| s.trim().parse().ok())
                .collect()
        })
    });
    spec.as_deref()
}

/// One frame's bone trace: positions of the whole skeleton, then axes/keying for the named joints.
fn bone_trace_frame(
    detail: &[usize],
    world_id: u32,
    selected: DatId,
    registered_slots: &[Option<(DatId, bool)>; 8],
    coordinator: &SkeletonAnimationCoordinator,
    skeleton: &ffxi_dat::skel::Skeleton,
    world_pose: &[Mat4],
) {
    let mut joints = String::new();
    for (index, m) in world_pose.iter().enumerate() {
        let t = m.w_axis;
        let _ = std::fmt::Write::write_fmt(
            &mut joints,
            format_args!("{index}:({:.3},{:.3},{:.3}) ", t.x, t.y, t.z),
        );
    }
    let mut slots = String::new();
    for (slot, anim) in registered_slots.iter().enumerate() {
        if let Some((id, battle)) = anim {
            let _ = std::fmt::Write::write_fmt(
                &mut slots,
                format_args!("{slot}:{}/{} ", id.as_str(), battle),
            );
        }
    }
    // The standard references name themselves once so a joint index in the log can be read without
    // opening the DAT (`ffxi-dat/src/skel.rs` `standard_position`).
    let mut refs = String::new();
    for slot in [
        ffxi_dat::skel::standard_position::NECK,
        ffxi_dat::skel::standard_position::CHEST,
        ffxi_dat::skel::standard_position::LEFT_HAND,
        ffxi_dat::skel::standard_position::RIGHT_HAND,
    ] {
        if let Some(r) = skeleton.reference_at(slot) {
            let _ = std::fmt::Write::write_fmt(&mut refs, format_args!("{slot}:j{} ", r.index));
        }
    }
    tracing::debug!(
        target: "bone",
        "BONE id={:#x} sel={} slots=[{}] refs=[{}] {joints}",
        world_id,
        selected.as_str(),
        slots.trim_end(),
        refs.trim_end(),
    );

    let expanded;
    let detail: &[usize] = if detail.contains(&BONE_LOG_ALL) {
        expanded = (0..world_pose.len()).collect::<Vec<_>>();
        &expanded
    } else {
        detail
    };
    for &joint in detail {
        let Some(m) = world_pose.get(joint) else {
            continue;
        };
        let mut keyed_by = String::new();
        for animator in coordinator.animations.iter().flatten() {
            let Some(ctx) = animator.current_animation.as_ref() else {
                continue;
            };
            if ctx.animation.key_frame_sets.contains_key(&(joint as u32)) {
                let _ = std::fmt::Write::write_fmt(
                    &mut keyed_by,
                    format_args!("{} ", ctx.animation.id.as_str()),
                );
            }
        }
        let t = m.w_axis;
        // The axes read in `POSE_FORWARD` pose space: +X forward, -Y up.
        let (col0, col1, col2) = (m.x_axis, m.y_axis, m.z_axis);
        tracing::debug!(
            target: "bone",
            "BONE_DET id={:#x} j={} bind_t=({:.3},{:.3},{:.3}) pose_t=({:.3},{:.3},{:.3}) fwd=({:.2},{:.2},{:.2}) up=({:.2},{:.2},{:.2}) side=({:.2},{:.2},{:.2}) keyed_by=[{}]",
            world_id,
            joint,
            skeleton.joints[joint].translation[0],
            skeleton.joints[joint].translation[1],
            skeleton.joints[joint].translation[2],
            t.x, t.y, t.z,
            col0.x, col0.y, col0.z,
            -col1.x, -col1.y, -col1.z,
            col2.x, col2.y, col2.z,
            keyed_by.trim_end(),
        );
    }
}

/// One frame of what the look-at law consumes: the rotation skinning composes this actor's `world_pose` with,
/// the actor's world position, and the target's attach-3 point in world space. Only these enter
/// [`look_point_actor_local`].
pub(crate) struct LookAtInput {
    pub pose_rotation: Quat,
    pub actor_world: Vec3,
    pub target_attach_world: Vec3,
}

/// The actor-local space retail keeps its chased look point in (`model+0xB0..B8`).
/// `FFXiMain.dll retail-2026-09` fetches the target's attach 3 in world space (`[vt+0x1C4](3)` at
/// RVA 0xD5C64, helper RVA 0xD4560), drops it by `1.2f` when the visibility WORD is set and stores it at
/// actor+0x848/0x84C/0x850 (RVA 0xD5C92..0xD5C9E); then RVA 0xD5CDA..0xD5D25 turns it into the value the bend
/// actually uses: subtract `*vt+0x1BC` (RVA 0xA4740 = `lea eax,[ecx+0x5fc]`, the actor position) through the
/// in-place subtract helper RVA 0x270a0, build an identity matrix with zeroed rotation (`0x27990` / `0x279b0`),
/// rotate it by **−yaw** about Y — `fld [eax+4] / fchs` on the pointer from `vt+0x1C0` (RVA 0xA4750 =
/// `lea eax,[ecx+0x61c]`, whose `.y` is actor+0x620) through helper RVA 0x27bd0 — and transform the point
/// (helper RVA 0x28200). So: world offset, yaw removed, nothing else. kuluu's heading lives on the entity transform
/// (`scene.rs` `heading_to_quat`, slerped toward it for self), so `pose_rotation` is that frame's rotation and this
/// takes it out exactly once; `scale` divides because kuluu folds model scale into pose space, where retail composes
/// it later.
pub(crate) fn look_point_actor_local(
    pose_rotation: Quat,
    actor_scale: f32,
    actor_world: Vec3,
    target_attach_world: Vec3,
) -> Vec3 {
    let offset = (target_attach_world - actor_world) / actor_scale.max(f32::EPSILON);
    pose_rotation.inverse() * offset
}

/// Pose space, measured from the real skeletons (`examples/zz-head-axis`, all races): every humanoid
/// stands with up `-Y` and faces `+X`. The look point retail chases is kept in this space.
const POSE_FORWARD: Vec3 = ffxi_actor::look_bend::POSE_FORWARD;

/// `KULUU_BONE_LOG=all`: one entry standing for every joint of each posed skeleton.
const BONE_LOG_ALL: usize = usize::MAX;

/// Release is positional, never angular: horizontal distance at or under 0.3
/// (`FFXiMain.dll retail-2026-09 RVA 0xD5DED`, `.rdata` `0x32B15C`).
const LOOK_RELEASE_HORIZON: f32 = 0.3;
/// ... or a forward component past half a unit behind the shoulder line (`FFXiMain.dll
/// retail-2026-09 RVA 0xD5DBF`, `-0.5f` from `.rdata`).
const LOOK_RELEASE_FORWARD: f32 = -0.5;
/// The blend weight opens and closes at ±0.04 per frame (`FFXiMain.dll retail-2026-09`: the immediate
/// `0x3d23d70a` written at RVA 0xD5D33). A second arm replaces it with a percent: when the gate call
/// `0x87060` passes and `[vt+0x144]()` returns non-zero (RVA 0xD5D44..0xD5D70), the step becomes
/// `[vt+0x144]() × .rdata 0x10329A18 (= 0.01) × .rdata 0x1032a85c (= 0.04)`; `vt+0x144` is the getter at
/// RVA 0x84B80 = `[actor+0x70] → [+0xE0]`, which defaults to **100** when there is no such object, so the arm
/// reproduces the same 0.04 by default and kuluu has no producer for a different percentage.
const HEAD_LOOK_WEIGHT_PER_FRAME: f32 = 0.04;
/// The look point chases the target at 1/32 of what is left per tick (`FFXiMain.dll
/// retail-2026-09`: the bend at RVA 0x2AC60 pushes `0x3d000000` at RVA 0x2AD71).
const HEAD_LOOK_CHASE_FRACTION: f32 = 1.0 / 32.0;
/// At weight 0 the look point resets to straight ahead on a 20-unit point (`FFXiMain.dll
/// retail-2026-09` default block at `.rdata`).
const HEAD_LOOK_NEUTRAL_AHEAD: f32 = 20.0;

/// The two-stage settle: [`HeadLook::weight`] opens the gate, [`HeadLook::chased_pose`] moves the
/// point retail keeps at `model+0xB0..B8`.
#[derive(Debug, Clone)]
struct HeadLook {
    weight: f32,
    chased_pose: Vec3,
}

impl Default for HeadLook {
    fn default() -> Self {
        Self {
            weight: 0.0,
            chased_pose: POSE_FORWARD * HEAD_LOOK_NEUTRAL_AHEAD,
        }
    }
}

impl HeadLook {
    fn advance(&mut self, aiming: bool, target_pose: Option<Vec3>, elapsed_frames: f32) {
        let step = HEAD_LOOK_WEIGHT_PER_FRAME * elapsed_frames;
        self.weight = if aiming {
            (self.weight + step).min(1.0)
        } else {
            (self.weight - step).max(0.0)
        };
        if let Some(target) = target_pose {
            self.chased_pose += (target - self.chased_pose) * HEAD_LOOK_CHASE_FRACTION;
        }
        if self.weight == 0.0 {
            self.chased_pose = POSE_FORWARD * HEAD_LOOK_NEUTRAL_AHEAD;
        }
    }
}

/// A target whose visibility WORD is not clear gets its look-at point dropped this much (`FFXiMain.dll
/// retail-2026-09 RVA 0xD5C84` subtracts the `1.2f` at `.rdata` RVA 0x32A404 from the attach-point Y).
const LOOK_POINT_LOWER_Y: f32 = 1.2;

/// Retail's look point for a target: attach point 3 in world space, lowered when that actor's
/// visibility WORD has any bit set (`FFXiMain.dll retail-2026-09` whole-word test at RVA 0xD5C7A).
fn target_look_point(neck_world: Vec3, look_at_visibility_word: u16) -> Vec3 {
    if look_at_visibility_word == 0 {
        neck_world
    } else {
        neck_world - Vec3::Y * LOOK_POINT_LOWER_Y
    }
}

/// Retail's release test, on the look point in actor-local space (forward `+X`): inside the horizon,
/// or behind the shoulder line.
fn look_at_release(look_point_pose: Vec3) -> bool {
    let horizontal = Vec2::new(look_point_pose.x, look_point_pose.z).length();
    horizontal <= LOOK_RELEASE_HORIZON || look_point_pose.x <= LOOK_RELEASE_FORWARD
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
mod actor_reveal_tests {
    use super::*;

    #[test]
    fn removing_an_arrival_restores_opacity_and_the_opaque_material() {
        let mut app = App::new();
        app.init_resource::<FfxiSkinRegistry>()
            .init_resource::<Assets<FfxiSkinnedMaterial>>()
            .add_observer(finish_actor_reveal);
        let slot = app
            .world_mut()
            .resource_mut::<FfxiSkinRegistry>()
            .alloc_instance(FfxiInstance {
                reveal: 0.25,
                opacity: 0.25,
                ..default()
            });
        let skeleton = Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        };
        let mut actor = render_actor_for_test(skeleton, Vec::new());
        actor.instance_slots.push(slot);
        let root = app.world_mut().spawn(actor).id();
        let opaque = app
            .world_mut()
            .resource_mut::<Assets<FfxiSkinnedMaterial>>()
            .add(FfxiSkinnedMaterial {
                base_color_texture: None,
                fading: false,
            });
        let fading = app
            .world_mut()
            .resource_mut::<Assets<FfxiSkinnedMaterial>>()
            .add(FfxiSkinnedMaterial {
                base_color_texture: None,
                fading: true,
            });
        let child = app
            .world_mut()
            .spawn((
                ChildOf(root),
                MeshMaterial3d(fading),
                ActorFadeMaterial(opaque.clone()),
            ))
            .id();
        let wire = app
            .world_mut()
            .spawn(crate::components::MorphIn {
                elapsed: 0.1,
                actor_root: root,
                enhanced: false,
            })
            .id();
        app.world_mut()
            .entity_mut(wire)
            .remove::<crate::components::MorphIn>();
        app.world_mut().flush();
        let mut registry = app.world_mut().resource_mut::<FfxiSkinRegistry>();
        assert_eq!(registry.instance_mut(slot).reveal, 1.0);
        assert_eq!(registry.instance_mut(slot).opacity, 1.0);
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<FfxiSkinnedMaterial>>(child)
                .unwrap()
                .0,
            opaque
        );
        assert!(app.world().get::<ActorFadeMaterial>(child).is_none());
    }
}

#[cfg(test)]
mod cutscene_transpar_tests {
    use super::*;

    fn transpar_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<FfxiSkinRegistry>()
            .add_systems(Update, tick_cutscene_transpar);
        app
    }

    fn spawn_faded_actor(app: &mut App, opacity: f32, end: f32, total_secs: f32) -> (Entity, u32) {
        let slot = app
            .world_mut()
            .resource_mut::<FfxiSkinRegistry>()
            .alloc_instance(FfxiInstance {
                opacity,
                ..default()
            });
        let skeleton = Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        };
        let mut actor = render_actor_for_test(skeleton, Vec::new());
        actor.instance_slots.push(slot);
        let root = app.world_mut().spawn(actor).id();
        let wire = app
            .world_mut()
            .spawn((FfxiRenderRoot(root), CutsceneTranspar::new(end, total_secs)))
            .id();
        (wire, slot)
    }

    /// The fade starts from the actor's first-tick opacity, interpolates to the
    /// end value, and removes itself on completion.
    #[test]
    fn the_transpar_fade_drives_opacity_and_releases() {
        let mut app = transpar_app();
        let (wire, slot) = spawn_faded_actor(&mut app, 0.8, 0.4, 2.0);

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        assert!(
            (app.world()
                .resource::<FfxiSkinRegistry>()
                .instance_opacity(slot)
                - 0.7)
                .abs()
                < 1e-5
        );
        assert!(app.world().get::<CutsceneTranspar>(wire).is_some());

        for _ in 0..4 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(0.5));
            app.update();
        }
        assert!(
            (app.world()
                .resource::<FfxiSkinRegistry>()
                .instance_opacity(slot)
                - 0.4)
                .abs()
                < 1e-5
        );
        assert!(
            app.world().get::<CutsceneTranspar>(wire).is_none(),
            "the fade must remove itself on completion"
        );
    }
}

#[cfg(test)]
mod look_at_visibility_word_tests {
    use super::*;

    #[test]
    fn an_actor_with_clear_visibility_flags_is_looked_at_at_its_neck() {
        let neck = Vec3::new(1.0, 2.5, -3.0);
        assert_eq!(target_look_point(neck, 0), neck);
    }

    #[test]
    fn any_visibility_flag_bit_lowers_the_look_point_by_1_2() {
        let neck = Vec3::new(1.0, 2.5, -3.0);
        for word in [0x1u16, 0x8000, i16::MIN as u16] {
            let lowered = target_look_point(neck, word);
            assert_eq!(lowered.y, neck.y - LOOK_POINT_LOWER_Y);
            assert_eq!(
                Vec3::new(lowered.x, 2.5, lowered.z),
                Vec3::new(1.0, 2.5, -3.0)
            );
        }
    }
}

#[cfg(test)]
mod head_look_tests {
    use super::*;

    /// The bend space is this actor's nose frame: the point is a world offset with that frame's rotation taken out
    /// once, so turning an actor together with its target changes nothing about what the law sees — and a target
    /// straight ahead stays on +X whatever way the body points.
    #[test]
    fn turning_the_actor_and_its_target_together_changes_nothing() {
        let actor_world = Vec3::new(7.0, 1.0, -3.0);
        for quarter_turns in 0..4 {
            let pose_rotation =
                Quat::from_rotation_y(quarter_turns as f32 * std::f32::consts::FRAC_PI_2);
            // Ahead of a nose-`+X` actor and slightly above the anchor: yaw-only in the nose frame.
            let in_pose_space = Vec3::new(4.0, -0.5, 0.0);
            let target_attach_world = actor_world + pose_rotation * in_pose_space;
            let local =
                look_point_actor_local(pose_rotation, 1.0, actor_world, target_attach_world);
            assert!(
                (local - in_pose_space).length() < 1e-4,
                "quarter turn {quarter_turns}: expected {in_pose_space}, got {local}"
            );
        }
    }

    /// The helper's law: a world offset un-rotated by exactly the rotation that frame composed with.
    #[test]
    fn the_look_point_unrotates_by_exactly_the_frames_pose_rotation() {
        let actor_world = Vec3::new(7.0, 1.0, -3.0);
        let facing = std::f32::consts::FRAC_PI_2;
        let pose_rotation = Quat::from_rotation_y(facing);
        // A target one unit along world +Z from the actor.
        let local = look_point_actor_local(pose_rotation, 1.0, actor_world, actor_world + Vec3::Z);
        assert!(
            (local.length() - 1.0).abs() < 1e-5,
            "rotation-only offset: {local}"
        );
        // Ry(-90deg) . +Z = -X: the target sits behind the nose once that yaw is removed.
        assert!(local.x < -0.99 && local.z.abs() < 1e-5, "{local}");
        // Scale divides (kuluu folds it into pose space), so a halved model keeps the authored offsets.
        let scaled = look_point_actor_local(
            Quat::IDENTITY,
            2.0,
            actor_world,
            actor_world + Vec3::X * 4.0,
        );
        assert!((scaled - Vec3::X * 2.0).length() < 1e-5, "{scaled}");
    }

    #[test]
    fn release_is_positional_not_angular() {
        // Inside the horizon (0.3) or past half a unit behind the shoulder line (-0.5 on +X):
        // both release, and neither test looks at an angle.
        assert!(look_at_release(Vec3::new(0.2, 0.0, 0.1)));
        assert!(look_at_release(Vec3::new(-0.6, 0.4, 0.9)));
        // A level target far off to the side is still held: no angle enters this test.
        assert!(!look_at_release(Vec3::new(1.0, 0.6, -2.0)));
        assert!(!look_at_release(Vec3::new(2.4, -1.2, 1.9)));
    }

    #[test]
    fn weight_opens_over_about_25_frames_and_closes_the_same_way() {
        let target = Vec3::new(4.0, 0.0, 0.0);
        let mut head = HeadLook::default();
        let mut open_frames = 0;
        while head.weight < 1.0 {
            head.advance(true, Some(target), 1.0);
            open_frames += 1;
            assert!(open_frames < 60, "weight stuck at {}", head.weight);
        }
        let mut close_frames = 0;
        while head.weight > 0.0 {
            head.advance(false, None, 1.0);
            close_frames += 1;
            assert!(close_frames < 60, "weight stuck at {}", head.weight);
        }
        // ±1 frame of the 25 that 0.04/frame implies at 60 fps.
        assert!((24..=27).contains(&open_frames), "opened in {open_frames}");
        assert!(
            (24..=27).contains(&close_frames),
            "closed in {close_frames}"
        );
    }

    #[test]
    fn the_look_point_chases_a_third_of_what_is_left_each_tick() {
        let target = Vec3::new(4.0, 0.0, 0.0);
        let mut head = HeadLook::default();
        let start = head.chased_pose;
        head.advance(true, Some(target), 1.0);
        let expected_gap = (target - start) * HEAD_LOOK_CHASE_FRACTION;
        assert!(
            (head.chased_pose - (start + expected_gap)).length() < 1e-5,
            "first chase step: {:?}",
            head.chased_pose
        );
        // ... and it converges on the target rather than snapping to it.
        for _ in 0..300 {
            head.advance(true, Some(target), 1.0);
        }
        assert!(
            (head.chased_pose - target).length() < 0.01,
            "did not settle: {:?}",
            head.chased_pose
        );
    }

    #[test]
    fn a_released_look_point_resets_to_straight_ahead() {
        let mut head = HeadLook::default();
        for _ in 0..200 {
            head.advance(true, Some(Vec3::new(4.0, 0.3, 1.0)), 1.0);
        }
        assert!(head.chased_pose.distance(Vec3::new(4.0, 0.3, 1.0)) < 0.05);
        for _ in 0..60 {
            head.advance(false, None, 1.0);
        }
        assert_eq!(head.weight, 0.0);
        assert_eq!(
            head.chased_pose,
            POSE_FORWARD * HEAD_LOOK_NEUTRAL_AHEAD,
            "retail resets the point to its straight-ahead default at weight 0"
        );
    }
}

// Bounds per-frame asset-add + entity-spawn cost when several loads finish at
// once (zone-in floods); the rest stay queued and drain on subsequent frames.
const ACTOR_SPAWNS_PER_FRAME: usize = 2;
/// HD images are indivisible; an oversized image gets a frame to itself.
const ACTOR_UPLOAD_BYTES_PER_FRAME: usize = 8 * 1024 * 1024;
/// Backpressure bounds decoded images waiting behind the upload budget.
const ACTOR_LOAD_PIPELINE_CAP: usize = 2;

struct ActorUpload {
    entity_id: u32,
    key: Option<ActorPrepKey>,
    prepared: Arc<PreparedActor>,
    pending: std::collections::VecDeque<Image>,
    handles: Vec<Handle<Image>>,
    bytes: usize,
}

fn upload_actor_images(
    upload: &mut ActorUpload,
    images: &mut Assets<Image>,
    used: &mut usize,
) -> bool {
    while let Some(image) = upload.pending.front() {
        let bytes = crate::gpu_assets::image_bytes(image);
        if *used > 0 && used.saturating_add(bytes) > ACTOR_UPLOAD_BYTES_PER_FRAME {
            return false;
        }
        let image = upload.pending.pop_front().unwrap();
        upload.handles.push(images.add(image));
        *used += bytes;
        upload.bytes += bytes;
    }
    true
}

#[derive(Resource, Default)]
pub struct ActorLoadInFlight {
    queued: std::collections::VecDeque<LoadActorRequest>,
    owners: HashMap<u32, Entity>,
    uploads: std::collections::VecDeque<ActorUpload>,
    tasks: HashMap<u32, Task<Result<PreparedLoad, String>>>,
    keys: HashMap<u32, ActorPrepKey>,
    ready: std::collections::VecDeque<(
        u32,
        Option<ActorPrepKey>,
        Arc<PreparedActor>,
        Vec<Handle<Image>>,
    )>,
    cache: ActorPrepCache,
}

impl ActorLoadInFlight {
    fn cancel(&mut self, entity_id: u32) {
        self.queued.retain(|req| req.entity_id != entity_id);
        self.tasks.remove(&entity_id);
        self.keys.remove(&entity_id);
        self.uploads.retain(|upload| upload.entity_id != entity_id);
        self.ready.retain(|(id, ..)| *id != entity_id);
    }
}

pub fn kick_load_actor_tasks(
    mut commands: Commands,
    mut events: MessageReader<LoadActorRequest>,
    tracked: Res<crate::scene::TrackedEntities>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut in_flight: ResMut<ActorLoadInFlight>,
    actor_root: Res<ActorDatRoot>,
    state: Option<Res<crate::snapshot::SceneState>>,
    positions: Query<&Transform, With<crate::components::WorldEntity>>,
) {
    let quality = crate::zone_texture::TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    let stale: Vec<u32> = in_flight
        .owners
        .iter()
        .filter(|&(id, owner)| tracked.by_id.get(id) != Some(owner))
        .map(|(&id, _)| id)
        .collect();
    for id in stale {
        in_flight.cancel(id);
        in_flight.owners.remove(&id);
    }
    let new_requests = !events.is_empty();
    for req in events.read() {
        let Some(&owner) = tracked.by_id.get(&req.entity_id) else {
            continue;
        };
        commands.entity(owner).remove::<Mesh3d>();
        in_flight.cancel(req.entity_id);
        in_flight.owners.insert(req.entity_id, owner);
        in_flight.queued.push_back(req.clone());
    }
    if new_requests {
        let self_id = state.as_ref().and_then(|state| state.snapshot.self_char_id);
        let origin = self_id
            .and_then(|id| tracked.by_id.get(&id))
            .and_then(|owner| positions.get(*owner).ok())
            .map(|tf| tf.translation);
        let distance = |id: u32| {
            tracked
                .by_id
                .get(&id)
                .and_then(|owner| positions.get(*owner).ok())
                .zip(origin)
                .map_or(f32::MAX, |(tf, origin)| {
                    tf.translation.distance_squared(origin)
                })
        };
        in_flight.queued.make_contiguous().sort_by(|a, b| {
            (Some(b.entity_id) == self_id)
                .cmp(&(Some(a.entity_id) == self_id))
                .then_with(|| distance(a.entity_id).total_cmp(&distance(b.entity_id)))
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
    }
    let queued_count = in_flight.queued.len();
    for _ in 0..queued_count {
        let req = in_flight.queued.pop_front().unwrap();
        let key = prep_key(&req.subject, quality);
        if let Some((prepared, image_handles)) = in_flight.cache.get_and_promote(&key) {
            in_flight
                .ready
                .push_back((req.entity_id, Some(key), prepared, image_handles));
            continue;
        }
        if in_flight.tasks.len() + in_flight.uploads.len() >= ACTOR_LOAD_PIPELINE_CAP
            || in_flight.keys.values().any(|active| active == &key)
            || in_flight
                .uploads
                .iter()
                .any(|upload| upload.key.as_ref() == Some(&key))
        {
            in_flight.queued.push_back(req);
            continue;
        }
        let subject = req.subject.clone();
        // The model's four authored CIB scales are selected by the entity's
        // GraphSize (research/XIClient/src/XIClient/source/World/Actor/
        // SkeletalMeshActor.cpp SkeletalMeshActor::GetCibScaleIndex, resolved by
        // research/XIClient/src/XIClient/source/CYy/Model/KzCibCollect.cpp
        // KzCibCollect::GetScale). PC and mount models take no scale here: their
        // CibCollect is merged from equipment CIBs, which this path does not
        // build. The chosen value bakes the bind pose/bounds here and rides
        // along to spawn_live_actor's per-frame RootTransform, so both see one
        // number.
        let npc_graph_size = match subject {
            ActorSubject::Npc { graph_size, .. } => Some(graph_size),
            _ => None,
        };
        let root_arc = actor_root.0.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let root = resolve_actor_root(root_arc)?;
            let mut loaded = match subject {
                ActorSubject::Mount { race } => load_mount_race(&root, race),
                ActorSubject::Npc { file_id, .. } => load_npc(&root, file_id),
                ActorSubject::Pc {
                    race,
                    mounted,
                    equipment,
                    body,
                    main_weapon,
                    sub_weapon,
                } => load_pc(
                    &root,
                    race,
                    mounted,
                    &equipment,
                    body,
                    main_weapon,
                    sub_weapon,
                ),
            }?;
            let scale = match (npc_graph_size, loaded.cib) {
                (Some(graph_size), Some(cib)) => cib.scale_factor(graph_size),
                _ => 1.0,
            };
            let (texture_names, images) =
                split_actor_textures(std::mem::take(&mut loaded.textures), quality);
            let parts = prepare_actor_parts(&loaded, texture_names, 0.0, scale);
            Ok(PreparedLoad {
                actor: PreparedActor {
                    loaded,
                    parts,
                    scale,
                },
                images,
            })
        });
        // Newest look wins: replacing the entry drops any stale in-flight load.
        in_flight.tasks.insert(req.entity_id, task);
        in_flight.keys.insert(req.entity_id, key);
        in_flight.ready.retain(|(id, ..)| *id != req.entity_id);
    }
}

pub fn poll_load_actor_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut material_cache: ResMut<FfxiSkinnedMaterialCache>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut images: ResMut<Assets<Image>>,
    tracked: Res<crate::scene::TrackedEntities>,
    entity_mesh: Option<Res<crate::scene::EntityMesh>>,
    mut in_flight: ResMut<ActorLoadInFlight>,
    q_existing: Query<&FfxiRenderRoot>,
    gpu_assets: Option<Res<crate::gpu_assets::GpuAssetResidency>>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
) {
    if in_flight.tasks.is_empty() && in_flight.ready.is_empty() && in_flight.uploads.is_empty() {
        return;
    }
    // EntityMesh only exists once a scene is loaded; park finished tasks until then.
    let Some(_) = entity_mesh else {
        return;
    };
    let mut completed: Vec<(u32, Result<PreparedLoad, String>)> = Vec::new();
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
            Ok(PreparedLoad {
                actor,
                images: cpu_images,
            }) => {
                in_flight.uploads.push_back(ActorUpload {
                    entity_id,
                    key,
                    prepared: Arc::new(actor),
                    pending: cpu_images.into(),
                    handles: Vec::new(),
                    bytes: 0,
                });
            }
            Err(e) => {
                warn!("ffxi actor load failed (entity {entity_id}): {e}");
            }
        }
    }
    let mut uploaded_bytes = 0;
    while let Some(upload) = in_flight.uploads.front_mut() {
        if !upload_actor_images(upload, &mut images, &mut uploaded_bytes) {
            break;
        }
        let upload = in_flight.uploads.pop_front().unwrap();
        if let Some(key) = &upload.key {
            in_flight.cache.insert(
                key.clone(),
                Arc::clone(&upload.prepared),
                upload.handles.clone(),
                upload.bytes,
            );
        }
        in_flight.ready.push_back((
            upload.entity_id,
            upload.key,
            upload.prepared,
            upload.handles,
        ));
    }
    if uploaded_bytes > 0 {
        tracing::debug!(target: "actor_upload", bytes = uploaded_bytes, queued = in_flight.queued.len(), uploading = in_flight.uploads.len(), ready = in_flight.ready.len(), "actor textures published");
    }
    let ready_count = in_flight.ready.len();
    let mut spawned = 0;
    for _ in 0..ready_count {
        if spawned >= ACTOR_SPAWNS_PER_FRAME {
            break;
        }

        let Some((entity_id, key, prepared, image_handles)) = in_flight.ready.pop_front() else {
            break;
        };
        if gpu_assets
            .as_ref()
            .is_some_and(|gpu| !gpu.images_ready(&image_handles))
        {
            in_flight
                .ready
                .push_back((entity_id, key, prepared, image_handles));
            continue;
        }
        spawned += 1;
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
            &image_handles,
            &mut materials,
            &mut material_cache,
            &mut registry,
            &prepared,
            wire_entity,
            entity_id,
            prepared.scale,
            settings.enhanced_actor_arrival,
        );

        commands.entity(wire_entity).remove::<Mesh3d>();
        commands
            .entity(wire_entity)
            .try_insert(FfxiRenderRoot(root));
        commands
            .entity(wire_entity)
            .try_insert(crate::components::MorphIn {
                elapsed: 0.0,
                actor_root: root,
                enhanced: settings.enhanced_actor_arrival,
            });

        // Animated mob extents need their own camera/picking policy (BakedActor, scene.rs);
        // locator metadata is independent.
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

/// A 0x6C TRANSPAR fade running on a wire entity (ffxi-event/src/cue.rs):
/// drives every instance slot's opacity to `end` over `total_secs`, capturing
/// the start value on the first tick the actor's slots exist, so a model that
/// lands mid-fade fades from whatever it arrives at. Removed on completion.
#[derive(Component)]
pub struct CutsceneTranspar {
    pub end: f32,
    pub total_secs: f32,
    pub elapsed: f32,
    start: Option<f32>,
}

impl CutsceneTranspar {
    pub fn new(end: f32, total_secs: f32) -> Self {
        Self {
            end,
            total_secs,
            elapsed: 0.0,
            start: None,
        }
    }
}

/// Drives the 0x6C TRANSPAR fades queued by a running cutscene:
/// ffxi-event/src/cue.rs Transpar. The query waits on the render root, so a
/// model still loading starts its fade once it lands.
pub fn tick_cutscene_transpar(
    time: Res<Time>,
    mut commands: Commands,
    mut q_fade: Query<(Entity, &mut CutsceneTranspar, &FfxiRenderRoot)>,
    q_actor: Query<&FfxiRenderActor>,
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    for (wire_entity, mut fade, root) in &mut q_fade {
        fade.elapsed += time.delta_secs();
        let Ok(actor) = q_actor.get(root.0) else {
            continue;
        };
        let slots = actor.instance_slots();
        if slots.is_empty() {
            continue;
        }
        let start = match fade.start {
            Some(start) => start,
            None => {
                let start = registry.instance_opacity(slots[0]);
                fade.start = Some(start);
                start
            }
        };
        let progress = (fade.elapsed / fade.total_secs).clamp(0.0, 1.0);
        let opacity = start + (fade.end - start) * progress;
        for &slot in slots {
            registry.set_instance_opacity(slot, opacity);
        }
        if progress >= 1.0 {
            commands.entity(wire_entity).remove::<CutsceneTranspar>();
        }
    }
}

// .agents/skills/retail-observe/references/2026-09-14-pc-model-arrival.md
const MORPH_DURATION: f32 = 32.0 / 60.0;

pub fn tick_morph_in(
    time: Res<Time>,
    mut commands: Commands,
    mut q_morph: Query<(Entity, &mut crate::components::MorphIn)>,
    q_actor: Query<&FfxiRenderActor>,
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    for (wire_entity, mut morph) in &mut q_morph {
        morph.elapsed += time.delta_secs();
        let progress = (morph.elapsed / MORPH_DURATION).clamp(0.0, 1.0);
        if let Ok(actor) = q_actor.get(morph.actor_root) {
            for &slot in actor.instance_slots() {
                if morph.enhanced {
                    registry.set_instance_reveal(slot, progress);
                } else {
                    registry.set_instance_opacity(slot, progress);
                }
            }
        }
        if progress >= 1.0 {
            commands
                .entity(wire_entity)
                .remove::<crate::components::MorphIn>();
        }
    }
}

pub fn finish_actor_reveal(
    trigger: On<Discard, crate::components::MorphIn>,
    q: Query<&crate::components::MorphIn>,
    actors: Query<(&FfxiRenderActor, &Children)>,
    mut materials: Query<(&ActorFadeMaterial, &mut MeshMaterial3d<FfxiSkinnedMaterial>)>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut commands: Commands,
) {
    let Ok(morph) = q.get(trigger.event().event_target()) else {
        return;
    };
    if let Ok((actor, children)) = actors.get(morph.actor_root) {
        for &slot in actor.instance_slots() {
            registry.set_instance_reveal(slot, 1.0);
            registry.set_instance_opacity(slot, 1.0);
        }
        for child in children {
            if let Ok((opaque, mut material)) = materials.get_mut(*child) {
                material.0 = opaque.0.clone();
                // A zone change despawns these children in the same frame the
                // fade finishes (scene.rs sync_entities_system); a removal on
                // a gone entity is nothing to warn about.
                commands.entity(*child).try_remove::<ActorFadeMaterial>();
            }
        }
    }
}

// Map an observed entity's broadcast animation byte (server_status / ANIMATIONTYPE)
// to its persistent rest pose. `/heal` and `/sit` ride the same animation channel
// the server uses for engage and fishing; SITCHAIR is left unmapped (needs a
// chair-anchored clip). vendor/server/data/enums/animation.yaml.
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
    name: Option<String>,
    /// The entity's wire animation byte (`enum ANIMATIONTYPE`,
    /// vendor/server/data/enums/animation.yaml). A mount actor has no entry of its own and carries its
    /// rider's byte.
    animation: u8,
    // Head-look: facetarget is a targid (act_index), so resolve it to the world_id
    // the position maps are keyed by. Distinct from bt_target_id (the combat-claim
    // UniqueNo), which only turns the head mid-combat and lives in a different
    // id-space — see vendor/server char_update.cpp Flags0.facetarget.
    face_target: u16,
    // Engaged combat stance is the server's animation byte (ANIMATION_ATTACK),
    // set on every entity at engage and broadcast in the General block — see LSB
    // CBattleEntity::OnEngage, vendor/server/src/map/entities/battle_entity.h. The
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
    /// This entity's half of the look-at gate chain: whether its stamped record Type makes it aimable
    /// at all (`crate::look_at_gates::look_at_target_record_allows`, over the byte retail stamps at
    /// `ent+0xEE` from the 0x0E `look.size`; doors, lifts and ships are never look-at targets).
    /// Read when an *observer* resolves its look point, so a door in the way of a head turn releases
    /// the aim instead of bending towards it.
    look_at_target_record_allowed: bool,
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
/// [`LiveSnapshotIndex`]: the modules that schedule this system with `.before()`/`.after()` read
/// the Local parameter type, so it is visible to them. The two `prev_` fields are cross-snapshot
/// memory, not per-frame scratch: they persist across frames and are pruned only on despawn.
#[derive(Default)]
pub struct FrameScratch {
    /// Each rendered actor's attach point 3 in world space — the look-at target (FFXiMain.dll
    /// retail-2026-09 RVA 0xD5C86 reads `[target_vt + 0x1C4](3)`).
    neck_look_point: HashMap<u32, Vec3>,
    mount_attach: HashMap<u32, MountAttach>,
    /// The entity hp_pct last observed per entity id. A 0 -> >0 transition on the next snapshot
    /// is a Raise and clears that entity's Defeated latch (DeadFromAction).
    prev_hp: HashMap<u32, Option<u8>>,
    /// Self's deadness as of the last snapshot (party row / homepoint timer channel; self's own
    /// entity hp_pct only updates when CHAR_PC carries UPDATE_HP). A true -> false transition is
    /// a Raise of self.
    prev_self_dead: Option<bool>,
    /// This pass's live entity ids and raised set, cleared at the top of each changed-snapshot
    /// pass instead of allocated per frame (the neck_look_point/mount_attach pattern below).
    live_ids: std::collections::HashSet<u32>,
    raised: std::collections::HashSet<u32>,
    /// Routines queued this frame onto entities that had no ActiveSchedulers yet; flushed after
    /// the special-pose loop so same-batch queues merge instead of overwriting. The system sits
    /// at Bevy's 16-parameter limit, so this rides in FrameScratch rather than as a Local.
    pending_routine_inserts:
        std::collections::HashMap<Entity, Vec<crate::scheduler_runtime::ActiveScheduler>>,
}

/// Status effects that freeze the character's animations, so a swing in
/// progress can't be sheathed: the server's `HasPreventActionEffect` set
/// (vendor/server/src/map/status_effect_container.cpp) — sleep, sleep II,
/// petrify, lullaby, charm, charm II, penalty, stun, terror. Icon id equals
/// effect id (LSB assigns icon == effect id). While one is on self the weapon
/// holds drawn even if the target dies; it sheathes once the effect lapses.
const ANIMATION_LOCK_EFFECTS: &[u16] = &[2, 7, 10, 14, 17, 19, 28, 159, 193];

/// Whether any of self's status icons is an animation-locking effect.
fn has_animation_lock_effect(icons: &[u16]) -> bool {
    icons
        .iter()
        .any(|icon| ANIMATION_LOCK_EFFECTS.contains(icon))
}

/// Whether self's weapon is drawn: the server's own ATTACK byte AND (an active
/// main target OR an animation-locked status). The weapon does not go out with
/// no target to swing at; an animation-locked character is the exception, holding
/// the weapon until the effect lapses.
fn weapon_drawn(self_server_status: u8, has_active_target: bool, animation_locked: bool) -> bool {
    self_server_status == ffxi_proto::decode::animation::ATTACK
        && (has_active_target || animation_locked)
}

/// Runs after `scene::apply_invis_flag_system`: that system resets every skinned
/// model root's Visibility from the invis flag each frame, and this one must
/// win that write for entities the server has hidden via status INVISIBLE
/// (buried mobs).
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
    // every other entity's root stays owned by scene.rs apply_invis_flag_system (invis-flag
    // PCs). The fourth slot is the Defeated latch: a killing result starts the death path on
    // this frame instead of waiting for the next entity hp_pct; a raise clears it.
    mut q_actors: Query<(
        Entity,
        &mut FfxiRenderActor,
        &GlobalTransform,
        &mut Visibility,
        Option<&crate::scheduler_runtime::DeadFromAction>,
    )>,
    mut commands: Commands,

    mut prev_zone: Local<Option<Option<u16>>>,
    mut index: Local<LiveSnapshotIndex>,
    mut frame_scratch: Local<FrameScratch>,
    // Last-observed special-pose wire state per entity (ffxi-actor/src/actor_state.rs
    // SpecialPose). Persists across frames (a `Local`), so the transition can advance from the
    // last snapshot's state; rebuilt entries read their prior state here rather than resetting
    // to plain on every change.
    mut special_mem: Local<HashMap<u32, ffxi_actor::actor_state::SpecialPose>>,
    // The entity-level routine vecs (scheduler_runtime.rs ActiveSchedulers): the special-pose
    // routine firing below and the AnimationLock set built before the parallel pass both read
    // through this one query. Sixteen parameters (Bevy's fn-item arity limit); new state goes
    // into FrameScratch, not here.
    mut q_scheds: Query<&mut crate::scheduler_runtime::ActiveSchedulers>,
) {
    use ffxi_actor::actor_state::RestKind;

    frame_scratch.pending_routine_inserts.clear();
    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    let self_id = state.snapshot.self_char_id;

    // Self KO is unreliable via the entity hp_pct (only updated when CHAR_PC
    // carries UPDATE_HP) and via the party row (absent/stale when solo).
    // death_homepoint_secs is published from the CHAR_STATUS and LOGIN packets
    // (ffxi-proto/src/map.rs), both gated on hpp == 0. Hoisted above the
    // snapshot-change block so a raise transition can be detected there; the
    // pose pass below reads the same value.
    let self_dead = state.snapshot.death_homepoint_secs.is_some()
        || crate::snapshot::resolve_self(&state.snapshot.party, self_id)
            .map(|m| m.hp_pct == 0)
            .unwrap_or(false);

    // Special-pose effect routines queued by this frame's wire-state transitions (the sub ->
    // routine table is ffxi-actor/src/actor_state.rs special_routine), mirroring
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
        frame_scratch.live_ids.clear();
        // World ids whose entity hp_pct just went 0 -> >0 on this snapshot (a Raise):
        // scheduler_runtime.rs DeadFromAction - the wire owns death state again, so their
        // Defeated latch is cleared below.
        frame_scratch.raised.clear();
        let mut dead_now = std::collections::HashSet::<u32>::new();
        for e in &state.snapshot.entities {
            frame_scratch.live_ids.insert(e.id);
            let prev_hp = frame_scratch.prev_hp.get(&e.id).copied();
            frame_scratch.prev_hp.insert(e.id, e.hp_pct);
            if matches!(prev_hp, Some(Some(0))) && e.hp_pct.is_some_and(|p| p > 0) {
                frame_scratch.raised.insert(e.id);
            }
            if e.hp_pct == Some(0) {
                dead_now.insert(e.id);
            }
            let mounted = state.snapshot.mount_of(e).is_some();
            // Advance the special-pose wire state from last frame to this snapshot's
            // status/animationsub (ffxi-actor/src/actor_state.rs next_special_pose). A no-op
            // (stays plain) for entities with no sub and a visible status, so it is safe to run
            // for all of them.
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
            // A trigger changes the pose (ffxi-actor/src/actor_state.rs SpecialPoseStep), so a
            // changed-and-interesting state is the whole log condition: plain visible entities
            // with no sub do not appear here.
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
                    name: e.name.clone(),
                    animation: e.animation,
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
                    look_at_target_record_allowed: look_at_gates::look_at_target_record_allows(
                        e.look.as_ref(),
                    ),
                },
            );
            index.id_by_targid.insert(e.act_index, e.id);

            if mounted {
                index.by_id.insert(
                    crate::scene::mount_actor_id(e.id),
                    SnapshotActorState {
                        name: None,
                        animation: e.animation,
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
                        // A mount actor mirrors its rider's entity look, and retail has no separate
                        // record for a mount at all (the rider is the entity), so it inherits the
                        // rider's verdict rather than inventing one.
                        look_at_target_record_allowed: look_at_gates::look_at_target_record_allows(
                            e.look.as_ref(),
                        ),
                    },
                );
            }
        }
        // Self's raise arrives through the party row / homepoint timer channel instead of an
        // entity hp_pct (snapshot.rs resolve_self; see self_dead above): a true -> false
        // transition of self-deadness.
        if let Some(sid) = self_id {
            if frame_scratch.prev_self_dead == Some(true) && !self_dead {
                frame_scratch.raised.insert(sid);
            }
        }
        frame_scratch.prev_self_dead = Some(self_dead);

        // Latch the death path on entities whose hp_pct is 0 on this snapshot: consumers of
        // scheduler_runtime.rs DeadFromAction beyond the pose pass (remote grounding) read the
        // latch, and a kill that did not arrive as a BATTLE2 Defeated result has no other
        // signal. Mount actors carry synthetic ids disjoint from server ids, so they do not
        // match dead_now.
        if !dead_now.is_empty() {
            for (entity, actor, _, _, latch) in q_actors.iter() {
                if latch.is_none() && dead_now.contains(&actor.world_id) {
                    commands
                        .entity(entity)
                        .insert(crate::scheduler_runtime::DeadFromAction::default());
                }
            }
        }

        // Clear the Defeated latch on raised entities (scheduler_runtime.rs DeadFromAction).
        // Commands apply at end of system, so this frame's pose pass still sees the latch for
        // one more frame; from the next frame the wire's hp_pct owns death state again.
        if !frame_scratch.raised.is_empty() {
            for (entity, actor, _, _, latch) in q_actors.iter() {
                if latch.is_some() && frame_scratch.raised.contains(&actor.world_id) {
                    commands
                        .entity(entity)
                        .remove::<crate::scheduler_runtime::DeadFromAction>();
                }
            }
        }

        // Drop states for entities that despawned so the cache stays bounded. SAFETY: field
        // borrows go through a materialized &mut (the mount_attach_scratch pattern below) -
        // split borrows do not propagate through Bevy's Local deref when one side is captured by
        // a closure.
        let frame_scratch = &mut *frame_scratch;
        let live_ids = &frame_scratch.live_ids;
        special_mem.retain(|id, _| live_ids.contains(id));
    }

    // Fire the queued routines on their wire entities (scheduler_runtime.rs ActiveScheduler):
    // it carries a world-space Transform (the particle/sound origin) and is what the
    // stage-dispatch systems read `(Transform, Option<ActionAssets>)` off. The actor's own
    // ActionAssets hold this DAT's SEPs and dirt generators, so they ride along for resolution.
    for (world_id, routine) in special_routines {
        let Some(&wire_e) = tracked.by_id.get(&world_id) else {
            continue;
        };
        // Model not loaded yet (scheduler_runtime.rs effects_only): the clip still plays from
        // the pose pass; only the dirt and sound are lost. Acceptable degradation — the load
        // lands within a few frames.
        let Some((_, actor, _, _, _)) = q_actors
            .iter()
            .find(|(_, a, _, _, _)| a.world_id == world_id)
        else {
            continue;
        };
        let lookup = crate::scheduler_runtime::RoutineLookup::new().with_actor(actor.routines());
        let Some(active) =
            crate::scheduler_runtime::ActiveScheduler::effects_only(&lookup, &routine)
        else {
            continue;
        };
        // Insert-or-push like the other dispatchers (scheduler_runtime.rs enqueue_routine): a
        // pop-up `init` alongside a still-running dig `ini1` (or vice versa) runs concurrently in
        // retail; each carries the StopRoutine that stops the other, so the overlap resolves
        // through StopRoutine. The push path leaves the first writer's ActionAssets/ActionTarget
        // alone.
        crate::scheduler_runtime::enqueue_routine(
            &mut commands,
            wire_e,
            active.with_target(Some(wire_e)),
        );
        commands
            .entity(wire_e)
            .try_insert_if_new(actor.action_assets().clone())
            .try_insert_if_new(crate::scheduler_runtime::ActionTarget(None));
    }
    crate::scheduler_runtime::flush_active_scheduler_inserts(
        &mut frame_scratch.pending_routine_inserts,
        &mut q_scheds,
        &mut commands,
    );

    if special_log_enabled() {
        SPECIAL_LOG_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let index: &LiveSnapshotIndex = &index;
    // SAFETY: split borrows of the scratch fields - each `.field` access through the Local's
    // DerefMut would take its own whole-value mutable borrow, so materialize one plain `&mut`
    // first and borrow disjoint fields from it.
    let frame_scratch = &mut *frame_scratch;
    let neck_look_scratch = &mut frame_scratch.neck_look_point;
    let mount_attach_scratch = &mut frame_scratch.mount_attach;

    // The look-at point is the target's own attach 3 off its posed skeleton, which also puts it at
    // rendered (grounded) height: wire Y for a pathing NPC is a flat reference and diverges from the
    // model after snap_entities_to_mzb_floor_system.
    neck_look_scratch.clear();
    for (_, a, gt, _, _) in &q_actors {
        let Some(neck) = standard_joint_world_position(
            &a.world_pose,
            &a.skeleton,
            ffxi_dat::skel::standard_position::NECK,
        ) else {
            continue;
        };
        neck_look_scratch.insert(
            a.world_id,
            target_look_point(
                gt.to_matrix().transform_point3(neck),
                a.look_at_visibility_word,
            ),
        );
    }
    let look_point_by_id: &HashMap<u32, Vec3> = neck_look_scratch;

    // Where each rider's body has to be pinned, for the mounts that pin one.
    // Read off the mount actor's posed skeleton, which shares the rider's root
    // transform exactly (scene.rs pins the mount entity to the rider's), so the
    // joint needs no reframing. Taken from the pose the mount held last frame —
    // the two actors are posed in the same pass and a frame of lag on a seat is
    // not visible.
    mount_attach_scratch.clear();
    for (_, a, _, _, _) in &q_actors {
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

    // World ids whose running routine currently holds an AnimationLock (scheduler_runtime.rs
    // is_locked_now): while locked the pose pass does not release the routine's Motion clip - a
    // one-shot pins its end frame in the coordinator, so this is what holds buried/emerged poses
    // until the lock lapses. Built serially before the parallel pass; the set is read-only
    // inside it.
    let animation_locked: std::collections::HashSet<u32> = tracked
        .by_id
        .iter()
        .filter(|(_id, e)| q_scheds.get(**e).is_ok_and(|s| s.is_locked_now()))
        .map(|(id, _)| *id)
        .collect();

    // The 0x89 LockLookAt intervals each entity's running routines currently cover, keyed by world id and
    // read at the routine clock; the parallel pass below turns them into per-actor suppression tasks.
    let mut lock_look_at_intervals: HashMap<u32, Vec<LookAtLockInterval>> = HashMap::new();
    for (id, entry) in tracked.by_id.iter() {
        let Ok(scheds) = q_scheds.get(*entry) else {
            continue;
        };
        let intervals = scheds.lock_look_at_tasks_now();
        if !intervals.is_empty() {
            lock_look_at_intervals.insert(*id, intervals);
        }
    }

    // World ids whose Defeated latch is up but the `dead` routine's fall-over Motion has not
    // fired yet (scheduler_runtime.rs dead_fall_over_pending): hold idle across that gap instead
    // of flashing cor?. Same serial-build/read-in-parallel pattern as animation_locked.
    let dead_fall_pending: std::collections::HashSet<u32> = tracked
        .by_id
        .iter()
        .filter(|(_id, e)| q_scheds.get(**e).is_ok_and(|s| s.dead_fall_over_pending()))
        .map(|(id, _)| *id)
        .collect();

    // Self's combat stance is the server's own animation byte: it flips to
    // ATTACK on an accepted engage (the 0x058 push — the server never sends its
    // own 0x0E) and back to NONE on a disengage. The reactor goal only predicts
    // on send, so it can't be the source of truth: a "wait longer" rejection
    // must not draw the weapon. The byte is additionally gated on an active
    // main target: the weapon is never out with no target to swing at, so a
    // target that dies (auto_clear drops it) sheathes the weapon even if the
    // server's ATTACK byte lags the corpse. The one exception is an
    // animation-locked status (sleep/petrify/lullaby/charm/penalty/stun/terror
    // — the server's HasPreventActionEffect set, status_effect_container.cpp):
    // such a character is frozen mid-swing, so the weapon stays drawn until the
    // effect lapses and the lock clears, then sheathes on the next frame.
    let self_animation_locked = has_animation_lock_effect(&state.snapshot.status_icons);
    let self_server_engaged = weapon_drawn(
        state.snapshot.self_server_status,
        target.id.is_some(),
        self_animation_locked,
    );
    log_self_server_status_change(state.snapshot.self_server_status, self_server_engaged);
    let self_reactor_driven = self_pose_follows_reactor(state.snapshot.current_goal.as_ref());

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
    let (self_move_forward, self_move_strafe, self_move_moving, self_move_locked) = (
        self_move.forward,
        self_move.strafe,
        self_move.moving,
        self_move.locked,
    );

    let motion = &*motion;
    q_actors.par_iter_mut().for_each(
        |(_entity, mut actor, actor_global, mut vis, dead_from_action)| {
            let world_id = actor.world_id;
            if world_id == 0 {
                return;
            }

            let is_self = Some(world_id) == self_id;
            if is_self && anim_trace_enabled() {
                ANIM_TRACE_ID.store(world_id, std::sync::atomic::Ordering::Relaxed);
            }
            let snap = index.by_id.get(&world_id);

            if zone_changed || (!is_self && snap.is_none()) {
                reset_actor_pose_state(
                    &mut actor,
                    elapsed_frames,
                    snap.and_then(|s| s.name.as_deref()),
                );
                return;
            }

            // A mount actor has no motion of its own: it is pinned to its rider's
            // transform, so its gait must come from whatever drives the rider.
            let motion_id = snap.and_then(|s| s.motion_from).unwrap_or(world_id);
            let drives_from_self_input = Some(motion_id) == self_id;
            let sample = motion.sample(motion_id).unwrap_or_default();

            let engaged = if is_self {
                self_server_engaged
            } else {
                snap.map(|s| s.engaged).unwrap_or(false)
            };
            // A Defeated result latches the death path on this frame (.agents/skills/retail-observe/references/2026-09-09-wormwatch-runtime.md "First non-burrow routines"); the 0x0E hp_pct
            // takes over from there. While the `dead` routine is queued but its
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

            // Special-pose wire state. Self does not carry one (its root belongs to scene.rs
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
                    &actor.rejected_routines,
                    &actor.battle_clips,
                    &actor.animations,
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
            // The stride scale matches a ground stride: the Cib Info movement byte (vekien/
            // xi-model-viewer ui/js/dat/inspect.js MOVEMENT_TYPE) says Flying and
            // Sliding mobs have no walk/run stride to match, so their locomotion clips play at
            // the authored rate. Walking/Large carry the wire scale; Unset (no CIB or CIB_UNSET)
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
            actor.locked_on = drives_from_self_input && !self_reactor_driven && self_move_locked;
            let modes = actor.anim_modes;
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
                battle_mode: modes.battle,
                idle_mode: modes.idle,
                walking_mode: modes.walking,
                running_mode: modes.running,
                ..Default::default()
            };
            actor.wire_animation =
                snap.map_or(ffxi_proto::decode::animation::NONE, |s| s.animation);

            // Retail aims nothing until both look-at gates pass: the wire status byte alone can drop the
            // target, and a live 0x89 LockLookAt task does the same from an action (crate::look_at_gates).
            let wire_animation = actor.wire_animation;
            let look_at_suppressed = look_at_gates::advance_look_at_suppression(
                &mut actor.look_at_tasks,
                lock_look_at_intervals
                    .get(&world_id)
                    .map_or(&[][..], Vec::as_slice),
                Vec2::new(actor_global.translation().x, actor_global.translation().z),
                wire_animation,
            );

            let look_target_id = if is_self {
                self_target_id
            } else {
                snap.map(|s| s.face_target)
                    .filter(|&t| t != 0)
                    .and_then(|targid| index.id_by_targid.get(&targid).copied())
            };
            // The target's own record Type is the second half of retail's gate chain: a door, lift or
            // ship releases the aim exactly like a status gate miss does (it does not abort the pass -
            // the weight still ramps down), so it filters here rather than suppressing the whole actor.
            let look = (!look_at_suppressed)
                .then(|| {
                    look_target_id
                        .filter(|&tid| tid != world_id)
                        .filter(|&tid| {
                            index.by_id.get(&tid).is_some_and(|target_state| {
                                target_state.look_at_target_record_allowed
                            })
                        })
                        .and_then(|tid| look_point_by_id.get(&tid).copied())
                        // Retail's yaw removal must use the orientation the renderer composes this actor's pose with;
                        // that is this transform, not `FfxiRenderActor::facing_dir`.
                        .map(|target_world| LookAtInput {
                            pose_rotation: actor_global.rotation(),
                            actor_world: actor_global.translation(),
                            target_attach_world: target_world,
                        })
                })
                .flatten();
            // Which of retail's three look-at states this frame is, decided from the current state alone:
            // no resolved point ⇒ IdleNoTarget (the actor's own animation states keep their bones), gates
            // closed ⇒ Suppressed, otherwise Aiming.
            let look_state = look_at_gates::look_state_of(!look_at_suppressed, look.is_some());

            if is_self && actor.action.map(|a| a.cast_pose).unwrap_or(false) && !self_casting {
                actor.action = None;
                actor.action_clips.clear();
            }

            let mount_attach = mount_attach_by_rider.get(&world_id).copied();

            advance_actor_pose(
                &mut actor,
                elapsed_frames,
                look_state,
                look,
                mount_attach,
                animation_locked.contains(&world_id),
                snap.and_then(|s| s.name.as_deref()),
            );

            // Special-pose visibility: status INVISIBLE hides the model root outright (retail
            // destroys the actor on that byte; we keep one hidden actor instead). Nothing else
            // writes here: a completed one-shot holds its end frame pinned in the coordinator,
            // and retail does not hide on clip completion - a worm's buried dig pose is occluded
            // by terrain exactly as there. Every other root belongs to scene.rs
            // apply_invis_flag_system, which resets it each frame.
            if special.hidden {
                *vis = Visibility::Hidden;
            }
        },
    );

    for (_, actor, _, _, _) in &q_actors {
        registry.set_skin_joints(actor.skin_slot, &actor.world_pose);
    }

    if let Some(self_id) = self_id {
        if let Some((_, actor, _, _, _)) = q_actors
            .iter()
            .find(|(_, a, _, _, _)| a.world_id == self_id)
        {
            rest.observe_exit_clip(matches!(actor.rest_phase, RestPlayback::Stopping { .. }));
        }
    }
}

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
    pub(crate) fn suffix(&mut self, root: Option<&DatRoot>, spell_id: u32) -> Option<&'static str> {
        if !self.loaded {
            self.loaded = true;
            self.table = root.map(ffxi_dat::spell_info::SpellTable::open_from_root);
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
    motion: Res<crate::combat_stance::EntityMotion>,
    mut melee_travel: ResMut<crate::scheduler_runtime::MeleeTravel>,
    mut q_actors: Query<&mut FfxiRenderActor>,
    mut last_seen: Local<u64>,
    mut spell_suffix: ResMut<SpellSuffixCache>,
    actor_root: Res<ActorDatRoot>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for (event_index, ev) in events.recent_with_index().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
            animation,
            target_id,
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
        // An interrupted aim re-issues the ranged-start category carrying the
        // SkillInterrupt animation id (vendor/server/src/map/action/interrupts.cpp
        // RangedInterrupt); matching the animation drops the aim even for a
        // payload whose FourCC is not the "sp??" marker the check above keys on.
        let ranged_interrupt = action_kind == ffxi_proto::melee::CATEGORY_RANGED_START
            && animation == Some(ffxi_proto::melee::RANGED_INTERRUPT_ANIMATION);
        if start.is_some_and(|m| m.interrupt) || ranged_interrupt {
            // Only a pose that outlives its own clip needs dropping; a one-shot has already
            // finished by the time an interrupt could matter (interrupts.cpp MagicInterrupt).
            if actor.action.is_some_and(|a| a.held()) {
                actor.action = None;
            }
            continue;
        }
        let cast_routine_id = start.map(|m| DatId::from_name(&m.id));
        let cast_suffix = match (action_kind == MAGIC_START_CATEGORY, cast_routine_id) {
            (true, None) => spell_suffix.suffix(actor_root.0.as_deref(), action_id),
            _ => None,
        };
        // A FourCC start keeps its category's looping semantics
        // (vendor/server/src/map/enums/four_cc.h): the generic `cast`/`calg` loops its one Motion
        // stage until resolution, while `cate`/`cait` wind up and settle.
        let fourcc_looping = matches!(action_kind, 10 | 12);
        let is_start = matches!(action_kind, 7 | 9 | 10 | 12 | MAGIC_START_CATEGORY);
        match cast_routine_id
            .map(|id| (id, action_kind == MAGIC_START_CATEGORY || fourcc_looping))
            .or_else(|| action_routine(action_kind, action_id, cast_suffix, animation))
        {
            None => {
                if actor.action.is_some_and(|a| a.held()) {
                    actor.action = None;
                }
            }
            Some((mut routine, mut looping)) => {
                // A travelling attacker's swing has its own authored pose (`atf0/atl0/atr0/atb0`,
                // research/xim Actor.kt onAttackMainHand), so this pass takes that clip rather than the
                // standing limb one whenever the model carries it: the scheduler arm has already armed
                // that swing's sounds and damage for the same event, and a standing body under them is
                // the bug.
                if action_kind == ffxi_proto::melee::CATEGORY_BASIC_ATTACK {
                    let travel = crate::scheduler_runtime::melee_travel_for_event(
                        &mut melee_travel,
                        &motion,
                        event_index,
                        actor_id,
                        target_id,
                    );
                    // One answer for both consumers: this is the same `swing_routine_for` call the scheduler
                    // arm made for this event — same limb/travel/seed (`event_index`, stamped by the log so it
                    // does not depend on how many entries either reader drained). A live RNG here would put one
                    // swing's sounds and damage over another swing's body. The fallbacks are inside it:
                    // travelling routine the model lacks → limb routine; limb missing (no bti0/cti0/dti0 Motion
                    // clip, vendor/server/src/map/attack.h AttackAnimation) → `ati0`, which an unresolved lookup
                    // below still turns into the same silent skip it always was.
                    if let Some(anim) =
                        animation.and_then(ffxi_proto::melee::AttackAnimation::from_wire)
                    {
                        routine = DatId::from_name(&crate::scheduler_runtime::swing_routine_for(
                            anim,
                            travel,
                            event_index,
                            &|name: [u8; 4]| {
                                routine_motion_clip(
                                    &actor.routines,
                                    &actor.rejected_routines,
                                    DatId::from_name(&name),
                                )
                                .is_some()
                            },
                        ));
                        looping = false;
                    }
                }
                let clip_id =
                    match routine_motion_clip(&actor.routines, &actor.rejected_routines, routine) {
                        Some(id) => id,
                        // A mob DAT may lack the spell school's cast routine (cawh & co);
                        // the generic `cast` is the race-base fallback the start pose still
                        // reads.
                        None if action_kind == MAGIC_START_CATEGORY
                            && routine != DatId::from_str("cast") =>
                        {
                            match routine_motion_clip(
                                &actor.routines,
                                &actor.rejected_routines,
                                DatId::from_str("cast"),
                            ) {
                                Some(id) => id,
                                None => {
                                    tracing::debug!(
                                        target: "combat",
                                        actor_id,
                                        ?routine,
                                        "no cast clip on this model; start pose skipped"
                                    );
                                    continue;
                                }
                            }
                        }
                        None => {
                            tracing::debug!(
                                target: "combat",
                                actor_id,
                                action_kind,
                                ?routine,
                                "no motion clip for this action; pose skipped"
                            );
                            continue;
                        }
                    };

                let len = rest_clip_len_frames(&actor.battle_clips, clip_id)
                    .max(rest_clip_len_frames(&actor.animations, clip_id));
                let remaining = if looping {
                    CAST_TIMEOUT_FRAMES
                } else {
                    len.max(1.0)
                };
                // Only a start (vendor/server/src/map/enums/four_cc.h) holds a pose past its
                // wind-up; a swing or a completion motion is over when its clip is.
                let settle = (is_start && !looping)
                    .then(|| {
                        settle_motion_clip(
                            &actor.routines,
                            &actor.rejected_routines,
                            routine,
                            clip_id,
                        )
                    })
                    .flatten();
                actor.action = Some(ActionPlayback {
                    clip_id,
                    looping,
                    remaining,
                    num_loops: None,
                    transition_in: LOCOMOTION_XFADE_IN,
                    transition_out: LOCOMOTION_XFADE_OUT,
                    cast_pose: action_kind == MAGIC_START_CATEGORY,
                    settle,
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
    q_actors: Query<(&FfxiRenderActor, &GlobalTransform)>,
    collision: Res<crate::dat_mzb::MzbCollisionGeometry>,
    weather: Res<crate::weather::ZoneWeather>,
    clock: Res<crate::vana_time::VanaClock>,
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

    const MINUTES_PER_HOUR: f32 = 60.0;
    let minutes = (crate::sun_moon::vana_sky_from_clock(&clock).hour * MINUTES_PER_HOUR) as u32;
    for (actor, transform) in &q_actors {
        let mut lighting = lighting.clone();
        if let Some(record) = collision
            .lighting_at(transform.translation())
            .and_then(|ground| weather.sample_for_area(ground.area, minutes))
        {
            let (ambient, direction, color) =
                crate::sun_moon::actor_area_lighting(&record, minutes, zone_lighting.model_dir);
            lighting.ambient = ambient;
            lighting.dir0_dir = direction;
            lighting.dir0_color = color;
            lighting.dir1_dir = Vec4::ZERO;
            lighting.dir1_color = Vec4::ZERO;
        }
        registry.set_skin_lighting(actor.skin_slot, &lighting);
        for &slot in &actor.instance_slots {
            registry.set_instance_lighting_flags(slot, realistic, receive);
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
    ground: Option<crate::dat_mzb::GroundLighting>,
    count: usize,
    lights_len: usize,
    indices: Vec<u32>,
    positions: Vec<Vec3>,
}

impl ActorPointLightSelection {
    fn valid_for(
        &self,
        pos: Vec3,
        ground: Option<crate::dat_mzb::GroundLighting>,
        count: usize,
        lights: &[crate::zone_point_lights::ZonePointLight],
    ) -> bool {
        self.ground == ground
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
    collision: Res<crate::dat_mzb::MzbCollisionGeometry>,
    mut q_actors: Query<(&mut FfxiRenderActor, &GlobalTransform)>,
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    if active.lights.is_empty() {
        return;
    }
    let count = settings.model_light_count as usize;
    for (mut actor, gt) in &mut q_actors {
        let pos = gt.translation();
        let ground = collision.lighting_at(pos);
        let cached_valid = actor
            .point_light_selection
            .as_ref()
            .is_some_and(|sel| sel.valid_for(pos, ground, count, &active.lights))
            && !collision.is_changed();
        if !cached_valid {
            let indices = match ground {
                Some(ground) => crate::zone_point_lights::authored_point_light_indices(
                    &active.lights,
                    &ground.lights,
                ),
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
                ground,
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
            if settings.dynamic_lights.point_shadows_enabled() {
                crate::zone_point_lights::point_light_arrays_for(&active.lights, &sel.indices)
            } else {
                crate::zone_point_lights::actor_directional_point_light(
                    pos,
                    &active.lights,
                    &sel.indices,
                )
            };

        registry.set_skin_point_lights(actor.skin_slot, point_pos, point_color, point_atten);
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
mod actor_texture_tests {
    use super::*;
    use ffxi_dat::texture::TexFormat;

    fn tex(width: u32, height: u32) -> DecodedTexture {
        DecodedTexture {
            width,
            height,
            format_tag: TexFormat::Bgra32,
            rgba: vec![0xFF; (width as usize) * (height as usize) * 4],
        }
    }

    fn synth_loaded(textures: Vec<NamedTexture>) -> LoadedActor {
        LoadedActor {
            skeleton: Arc::new(Skeleton {
                id: DatId::from_str("0000"),
                joints: Vec::new(),
                references: Vec::new(),
                bounding_boxes: Vec::new(),
                look_at_limits: Vec::new(),
            }),
            skel_meshes: Vec::new(),
            effect_meshes: Vec::new(),
            textures,
            animations: Arc::new(Vec::new()),
            battle_clips: Arc::new(Vec::new()),
            routines: Arc::new(HashMap::new()),
            action_assets: Arc::new(crate::scheduler_runtime::ActionAssets::default()),
            rejected_clips: Vec::new(),
            rejected_routines: Vec::new(),
            model_dat: "test.DAT".to_string(),
            cib: None,
            weapon_handles: Vec::new(),
        }
    }

    #[test]
    fn actor_images_are_render_world_only() {
        let img =
            decoded_texture_to_image(tex(2, 2), crate::zone_texture::TextureQuality::default());
        assert_eq!(
            img.asset_usage,
            RenderAssetUsages::RENDER_WORLD,
            "actor texels are never read back on the CPU, so the main-world copy \
             must be dropped at upload"
        );
        assert_eq!(img.data.as_ref().expect("texels").len(), 16);
    }

    #[test]
    fn prepared_actor_retains_no_decoded_texture_bytes() {
        let mut loaded = synth_loaded(vec![NamedTexture {
            name: "nsxxxxxxtexture1".to_string(),
            texture: tex(2, 2),
        }]);
        let (names, images) = split_actor_textures(
            std::mem::take(&mut loaded.textures),
            crate::zone_texture::TextureQuality::default(),
        );
        let parts = prepare_actor_parts(&loaded, names, 0.0, 1.0);

        assert!(
            loaded.textures.is_empty(),
            "the cached PreparedActor must not keep decoded rgba alive"
        );
        assert_eq!(parts.texture_names, vec!["nsxxxxxxtexture1".to_string()]);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data.as_ref().expect("texels").len(), 16);
    }

    #[test]
    fn texture_names_and_images_stay_index_aligned() {
        let (names, images) = split_actor_textures(
            vec![
                NamedTexture {
                    name: "first".to_string(),
                    texture: tex(2, 2),
                },
                NamedTexture {
                    name: "second".to_string(),
                    texture: tex(4, 1),
                },
            ],
            crate::zone_texture::TextureQuality::default(),
        );
        assert_eq!(names, vec!["first".to_string(), "second".to_string()]);
        assert_eq!((images[0].width(), images[0].height()), (2, 2));
        assert_eq!(
            (images[1].width(), images[1].height()),
            (4, 1),
            "build_actor_children zips names against handles and zip truncates \
             silently, so a drift here renders the wrong texture, not an error"
        );
    }

    #[test]
    fn same_look_spawns_share_one_image_upload() {
        const TEXTURE_NAME: &str = "nsxxxxxxtexture1";

        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<FfxiSkinnedMaterial>::default();
        let mut material_cache = FfxiSkinnedMaterialCache::default();
        let mut registry = FfxiSkinRegistry::default();

        let parts = PreparedParts {
            texture_names: vec![TEXTURE_NAME.to_string()],
            skel_built: vec![BuiltGroup {
                mesh: Mesh::new(
                    PrimitiveTopology::TriangleList,
                    RenderAssetUsages::default(),
                ),
                texture_name: TEXTURE_NAME.to_string(),
                tint: Vec4::ONE,
                joint_aabbs: Vec::new().into(),
            }],
            d3m_built: Vec::new(),
            bind_joints: FfxiJointMatrices::default(),
            bounds: None,
        };
        let prepared = Arc::new(PreparedActor {
            loaded: synth_loaded(Vec::new()),
            parts,
            scale: 1.0,
        });

        let handle = images.add(decoded_texture_to_image(
            tex(2, 2),
            crate::zone_texture::TextureQuality::default(),
        ));
        let mut cache = ActorPrepCache::default();
        let key = ActorPrepKey::Npc {
            file_id: 1,
            graph_size: 0,
            mipmaps: false,
            anisotropy: 1,
        };
        cache.insert(key.clone(), prepared, vec![handle], 0);
        assert_eq!(images.len(), 1);

        let mut spawned_handles = Vec::new();
        for _ in 0..2 {
            let (hit, image_handles) = cache.get_and_promote(&key).expect("cached entry");
            let mesh_handles = cache.mesh_handles(&key, &mut meshes).expect("cached entry");
            let skin_slot = registry.alloc_skin();
            let mut state: bevy::ecs::system::SystemState<Commands> =
                bevy::ecs::system::SystemState::new(&mut world);
            let mut commands = state.get_mut(&mut world).expect("commands param");
            let root = commands.spawn_empty().id();
            build_actor_children(
                &mut commands,
                &mesh_handles,
                &image_handles,
                &mut materials,
                &mut material_cache,
                &mut registry,
                &hit.parts,
                root,
                skin_slot,
                None,
            );
            state.apply(&mut world);
            spawned_handles.push(image_handles);
        }

        assert_eq!(
            images.len(),
            1,
            "a second spawn of the same look must not add a new Image asset"
        );
        let ids: Vec<Vec<AssetId<Image>>> = spawned_handles
            .iter()
            .map(|hs| hs.iter().map(Handle::id).collect())
            .collect();
        assert_eq!(ids[0], ids[1], "both spawns must share one texture upload");
        assert_eq!(
            material_cache.len(),
            1,
            "FfxiSkinnedMaterialCache keys on AssetId<Image>, so one look must \
             collapse to one material across spawns"
        );
    }
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
            look_at_limits: Vec::new(),
        };
        let loaded = LoadedActor {
            weapon_handles: Vec::new(),
            skeleton: Arc::new(skeleton),
            skel_meshes: Vec::new(),
            effect_meshes: Vec::new(),
            textures: Vec::new(),
            animations: Arc::new(Vec::new()),
            battle_clips: Arc::new(Vec::new()),
            routines: Arc::new(HashMap::new()),
            action_assets: Arc::new(crate::scheduler_runtime::ActionAssets::default()),
            rejected_clips: Vec::new(),
            rejected_routines: Vec::new(),
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
                texture_names: Vec::new(),
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
            graph_size: 0,
            mipmaps: false,
            anisotropy: 1,
        }
    }

    fn upload(width: u32, count: usize) -> ActorUpload {
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        ActorUpload {
            entity_id: 1,
            key: Some(npc_key(1)),
            prepared: synth_prepared(0),
            pending: (0..count)
                .map(|_| {
                    Image::new_uninit(
                        Extent3d {
                            width,
                            height: width,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::RENDER_WORLD,
                    )
                })
                .collect(),
            handles: Vec::new(),
            bytes: 0,
        }
    }

    #[test]
    fn texture_budget_spans_looks_and_oversized_images_make_progress() {
        let mut images = Assets::default();
        let mut used = 0;
        let mut first = upload(1024, 3);
        assert!(!upload_actor_images(&mut first, &mut images, &mut used));
        assert_eq!(used, ACTOR_UPLOAD_BYTES_PER_FRAME);
        assert_eq!(first.handles.len(), 2);
        used = 0;
        assert!(upload_actor_images(&mut first, &mut images, &mut used));
        let mut oversized = upload(2048, 2);
        assert!(!upload_actor_images(&mut oversized, &mut images, &mut used));
        assert!(oversized.handles.is_empty());
        used = 0;
        assert!(!upload_actor_images(&mut oversized, &mut images, &mut used));
        assert_eq!(oversized.handles.len(), 1);
        assert!(used > ACTOR_UPLOAD_BYTES_PER_FRAME);
        used = 0;
        assert!(upload_actor_images(&mut oversized, &mut images, &mut used));
    }

    #[test]
    fn cache_evicts_by_texture_bytes_and_promotes_recent_looks() {
        let mut cache = ActorPrepCache::default();
        let half = ACTOR_PREP_CACHE_BYTES / 2;
        for id in [1, 2] {
            cache.insert(npc_key(id), synth_prepared(0), Vec::new(), half);
        }
        cache.get_and_promote(&npc_key(1)).unwrap();
        cache.insert(npc_key(3), synth_prepared(0), Vec::new(), half);
        assert!(cache.map.contains_key(&npc_key(1)));
        assert!(!cache.map.contains_key(&npc_key(2)));
        assert_eq!(cache.map.len(), 2);
        cache.insert(
            npc_key(4),
            synth_prepared(0),
            Vec::new(),
            ACTOR_PREP_CACHE_BYTES + 1,
        );
        assert!(cache.map.is_empty());
    }

    #[test]
    fn cancelling_a_look_drops_every_pending_stage() {
        let mut in_flight = ActorLoadInFlight::default();
        in_flight.queued.push_back(LoadActorRequest {
            entity_id: 1,
            subject: ActorSubject::Npc {
                file_id: 1,
                graph_size: 0,
            },
        });
        in_flight.keys.insert(1, npc_key(1));
        in_flight.uploads.push_back(upload(1, 1));
        in_flight
            .ready
            .push_back((1, Some(npc_key(1)), synth_prepared(0), Vec::new()));
        in_flight.cancel(1);
        assert!(
            in_flight.queued.is_empty()
                && in_flight.keys.is_empty()
                && in_flight.uploads.is_empty()
                && in_flight.ready.is_empty()
        );
    }

    #[test]
    fn cached_look_reuses_the_same_mesh_handles() {
        let mut meshes = Assets::<Mesh>::default();
        let mut cache = ActorPrepCache::default();
        let key = npc_key(1);
        cache.insert(key.clone(), synth_prepared(2), Vec::new(), 0);

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
        // The live path's layered resolution: the requested id first, then the idle family
        // (ffxi-actor/src/actor_state.rs idle_animation_id).
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
        let root = ffxi_dat::archive::open_test_install()?;

        Some(load_pc(&root, 1, false, &[], None, None, None).expect("load Hume M"))
    }

    const HUME_M: u8 = 1;
    /// The main-hand slot's model id for the dagger whose mesh binds every vertex to joint 10.
    const DAGGER_MODEL: u16 = 2;
    /// The Info standard-joint byte measured on that model (`ffxi_dat::cib::Cib::standard_joint`).
    const DAGGER_HANDLE_REFERENCE: usize = 0x74;

    /// Hume M with the dagger equipped, plus the single skeleton joint that dagger's own mesh binds
    /// every vertex to (read out of the model DAT, not assumed).
    fn load_hume_m_with_dagger() -> Option<(LoadedActor, usize)> {
        let root = ffxi_dat::archive::open_test_install()?;
        let dll = main_dll_for_root(root.root())?;
        let dagger = crate::look_resolver::equipment_dat_id(&dll, 6, DAGGER_MODEL, HUME_M)?;
        let mut bound: Vec<usize> = ResourceDir::from_bytes(read_dat(&root, dagger)?)
            .collect_skel_meshes()
            .iter()
            .flat_map(|m| m.meshes.iter())
            .flat_map(|b| b.vertices.iter().map(|v| v.joint_index0 as usize))
            .collect();
        bound.sort_unstable();
        bound.dedup();
        let [anchor] = bound.as_slice() else {
            return None;
        };
        let loaded = load_pc(&root, HUME_M, false, &[dagger], None, Some(dagger), None).ok()?;
        Some((loaded, *anchor))
    }

    /// Step the engage machine to Engaged and hold it there (`in 0`'s own length plus slack).
    fn advance_engaged(actor: &mut FfxiRenderActor, frames: usize) {
        actor.inputs = inputs_for_pose(PoseState::Idle, true);
        for _ in 0..frames {
            advance_engage(
                &mut actor.engage,
                true,
                &actor.routines,
                &actor.rejected_routines,
                &actor.battle_clips,
                &actor.animations,
                1.0,
            );
            advance_actor_pose_standalone(actor, 1.0, None);
        }
    }

    /// Retail re-parents a held weapon's handle joint onto the hand while the actor is engaged
    /// (research/xim resource/SkeletonInstance.kt computeJointParentOverrides), and drops that joint's own
    /// authored transform in the process. Pins which joint the law moves for a dagger - from the model's
    /// Info byte through this skeleton's reference table - and that it lands exactly on the hand frame.
    #[test]
    fn an_engaged_pc_holds_its_weapon_on_the_hand_not_on_its_own_chain() {
        let Some((loaded, anchor)) = load_hume_m_with_dagger() else {
            return;
        };
        let handle = loaded
            .skeleton
            .reference_at(DAGGER_HANDLE_REFERENCE)
            .expect("dagger handle reference")
            .index;
        let hand = loaded
            .skeleton
            .reference_at(ffxi_dat::skel::standard_position::RIGHT_HAND)
            .expect("right hand reference")
            .index;
        assert_ne!(anchor, handle);
        assert_eq!(loaded.skeleton.joints[anchor].parent, Some(handle));

        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        assert_eq!(actor.weapon_handles, vec![(handle, hand)]);

        for _ in 0..4 {
            actor.inputs = inputs_for_pose(PoseState::Idle, false);
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        let free = actor.world_pose();
        assert!(
            (free[handle].w_axis.truncate() - free[hand].w_axis.truncate()).length() > 0.5,
            "unengaged the handle joint rides its own chain; if it already sits on the hand the \n\
             engaged assertion below proves nothing"
        );

        advance_engaged(&mut actor, 45);
        assert!(matches!(actor.engage, EngageMachine::Engaged));
        let posed = actor.world_pose();
        let off = (posed[handle].w_axis.truncate() - posed[hand].w_axis.truncate()).length();
        assert!(
            off < 1e-5,
            "engaged: handle {handle} is {off:.5} from hand {hand}"
        );
        let axes = [
            (posed[handle].x_axis.dot(posed[hand].x_axis), "x"),
            (posed[handle].y_axis.dot(posed[hand].y_axis), "y"),
            (posed[handle].z_axis.dot(posed[hand].z_axis), "z"),
        ];
        for (dot, name) in axes {
            assert!(
                dot > 0.9999,
                "engaged: handle {handle} {name}-axis is {dot:.4} off hand {hand}"
            );
        }
    }

    /// The weapon must stay put while the strafe clips change hands underneath it. Their authored keys are
    /// 150-178 degrees apart on the handle joints, so any mesh still riding that chain sweeps across the
    /// body the frame a strafe direction flips (research/xim poc/ActorManager.kt needsInBetweenFrame).
    #[test]
    fn a_held_weapon_never_leaves_the_hand_across_a_strafe_direction_change() {
        let Some((loaded, anchor)) = load_hume_m_with_dagger() else {
            return;
        };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        let (handle, hand) = actor.weapon_handles[0];
        advance_engaged(&mut actor, 45);

        let mut settled_gap: Option<f32> = None;
        let mut phase = PoseState::StrafeLeft;
        for frame in 0..60 {
            if frame % 15 == 0 {
                phase = if matches!(phase, PoseState::StrafeLeft) {
                    PoseState::StrafeRight
                } else {
                    PoseState::StrafeLeft
                };
            }
            actor.inputs = inputs_for_pose(phase, true);
            advance_actor_pose_standalone(&mut actor, 1.0, None);
            let posed = actor.world_pose();
            let handle_off =
                (posed[handle].w_axis.truncate() - posed[hand].w_axis.truncate()).length();
            assert!(
                handle_off < 1e-5,
                "frame {frame}: handle {handle} drifted {:.5} off hand {hand}",
                handle_off
            );
            let gap = (posed[anchor].w_axis.truncate() - posed[hand].w_axis.truncate()).length();
            match settled_gap {
                Some(want) => assert!(
                    (gap - want).abs() < 1e-4,
                    "frame {frame}: mesh anchor {anchor} is {gap:.5} from the hand, was {want:.5}"
                ),
                None if frame >= 8 => settled_gap = Some(gap),
                _ => {}
            }
        }
        assert!(settled_gap.is_some(), "the script never settled a strafe");
    }

    /// Why the re-parent has to discard the handle joint's own keys: measured separations between the
    /// battle stance and each strafe clip on Hume M's handle joints.
    #[test]
    fn the_strafe_clips_put_the_weapon_handle_joints_half_a_turn_from_the_stance() {
        let Some(loaded) = load_hume_m() else { return };
        let battle = loaded.all_battle_clips();
        let clips = loaded
            .animations
            .iter()
            .chain(battle.iter())
            .collect::<Vec<_>>();
        let keys_of = |id: &str| {
            clips
                .iter()
                .find(|c| c.id.as_str() == id)
                .map(|c| c.key_frame_sets.clone())
        };
        let stance = keys_of("btl1").expect("battle stance clip");
        for strafe in ["mvl1", "mvr1"] {
            let strafe_keys = keys_of(strafe).expect("strafe clip");
            for handle in [loaded
                .skeleton
                .reference_at(DAGGER_HANDLE_REFERENCE)
                .expect("dagger handle")
                .index as u32]
            {
                let (a, b) = (
                    stance.get(&handle).map(|f| &f[0]),
                    strafe_keys.get(&handle).map(|f| &f[0]),
                );
                let (Some(a), Some(b)) = (a, b) else { continue };
                let dot = (a.rotation[0] * b.rotation[0]
                    + a.rotation[1] * b.rotation[1]
                    + a.rotation[2] * b.rotation[2]
                    + a.rotation[3] * b.rotation[3])
                    .abs()
                    .clamp(0.0, 1.0);
                let sep = 2.0 * dot.acos() * (180.0 / std::f32::consts::PI);
                assert!(
                    sep > 150.0,
                    "{strafe} puts handle joint {handle} only {sep:.1} deg from the battle stance"
                );
            }
        }
    }

    /// Shipped locomotion clips split the skeleton between their own slot digits: `xxx0` writes the
    /// lower-body set, its `xxx1` sibling the rest, and no bone appears in both — so per-bone layer
    /// ownership keeps legs and arms apart without any authored mask to configure.
    #[test]
    fn shipped_locomotion_clips_partition_their_keyed_joints_by_slot_digit() {
        let Some(loaded) = load_hume_m() else { return };
        let base = &loaded.animations[..];
        for family in ["idl", "wlk", "run"] {
            let lower = keyed_joints(base, &format!("{family}0"));
            let upper = keyed_joints(base, &format!("{family}1"));
            assert!(
                lower.len() >= 10 && upper.len() >= 40,
                "{family}: lower={lower:?} upper={upper:?}"
            );
            let shared: Vec<usize> = lower
                .iter()
                .copied()
                .filter(|j| upper.contains(j))
                .collect();
            assert!(shared.is_empty(), "{family} digits both write {shared:?}");
        }
    }

    /// A weapon-draw or sheathe clip carries its own copy of the standing lower body, so switching to it
    /// does not leave those bones on whatever locomotion layer was sampled last. Idle is the one clip of
    /// the set that never writes two of them.
    #[test]
    fn draw_and_sheathe_author_their_own_lower_body() {
        let Some(loaded) = load_hume_m() else { return };
        let battle = loaded.all_battle_clips();
        let standing = keyed_joints(&battle[..], "btl0");
        assert!(standing.len() >= 10, "{standing:?}");
        for id in ["ind0", "otd0"] {
            assert_eq!(keyed_joints(&battle[..], id), standing, "{id} vs btl0");
        }
        let idle = keyed_joints(&loaded.animations[..], "idl0");
        assert!(idle.iter().all(|j| standing.contains(j)), "{idle:?}");
        assert!(
            idle.len() < standing.len(),
            "idle writes as many bones as a battle stance: {idle:?} vs {standing:?}"
        );
    }

    fn keyed_joints(clips: &[SkeletonAnimation], id: &str) -> Vec<usize> {
        let mut keys: Vec<usize> = clips
            .iter()
            .filter(|c| c.id.as_str() == id)
            .flat_map(|c| c.key_frame_sets.keys().map(|k| *k as usize))
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    #[test]
    fn live_idle_keeps_animating_with_nonselector_flags() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        bevy::tasks::ComputeTaskPool::get_or_init(Default::default);
        let mut results = Vec::new();
        // vendor/server/data/zones/valkurm_dunes/mobs.yaml Damselfly
        // render.animation_sub 8, east_ronfaure/mobs.yaml Wild_Sheep 16,
        // Forest_Hare carries no animation_sub key (0).
        for (name, file_id, animationsub) in [
            ("Damselfly", 1748, 8),
            ("Sheep", 1640, 16),
            ("Hare", 1568, 0),
            ("Damselfly without a dedicated sp1 clip", 1748, 1),
        ] {
            let loaded = load_npc(&root, file_id).expect("installed retail NPC DAT");
            if animationsub == 1 {
                // sub=1 names the ini1 routine (ffxi-actor/src/actor_state.rs special_routine);
                // on burrowing models its dig motion clip is sp1?, and this model ships no such
                // clip, so the override falls through to idle.
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
            // sub here settles to plain locomotion. The wire slot is what holds a special pose
            // between packets (ffxi-actor/src/actor_state.rs SpecialPose slot_held); the load
            // routine's lock alone lapses on its own, so a non-selector sub must leave the slot
            // clear for the model to keep idle-animating.
            let healthy = moved && (animationsub == 1 || !actor.inputs.special.slot_held);
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        // Installed ROM/5/64.DAT, the tunnel worm reference (look_resolver.rs npc_dat_id).
        let loaded =
            load_npc(&root, crate::look_resolver::npc_dat_id(0x01a8)).expect("installed worm DAT");
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        // sub=1 names the ini1 routine (ffxi-actor/src/actor_state.rs special_routine); its
        // first Motion stage is the dig clip (the clip comes from the routine record, not a
        // hard-coded mapping). The wire slot is what holds the pose here: the standalone path
        // has no schedulers, so no lock is in effect and only the slot keeps the override
        // selected.
        let dig = routine_motion_clip(
            &actor.routines,
            &actor.rejected_routines,
            DatId::from_name(b"ini1"),
        )
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
        actor.inputs.special.slot_held = true;
        for _ in 0..(duration.ceil() as usize * 2 + 1) {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        assert!(actor
            .last_clip
            .is_some_and(|id| id.parameterized_match(&dig)));
        let buried_pose = actor.world_pose().to_vec();
        // The one-shot holds its end frame (ffxi-actor/src/animation.rs
        // SkeletonAnimationCoordinator): no further advance moves the pose.
        advance_actor_pose_standalone(&mut actor, duration, None);
        assert_eq!(actor.world_pose(), buried_pose);
    }

    #[test]
    fn nameplate_locators_match_installed_retail_dat_measurements() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        // Nameplate offsets are the ABOVE_HEAD skeleton reference (index 2,
        // ffxi-dat/src/skel.rs standard_position), measured from the installed DATs
        // (identical on horizonxi-2023 and retail-2026-09).
        let models = [
            (
                "Tarutaru",
                load_pc(&root, 5, false, &[], None, None, None).unwrap(),
                1.3,
            ),
            (
                "Galka",
                load_pc(&root, 8, false, &[], None, None, None).unwrap(),
                2.6,
            ),
            ("Damselfly", load_npc(&root, 1748).unwrap(), 3.5),
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut heights: std::collections::HashMap<u8, f32> = std::collections::HashMap::new();
        for race in 1..=8u8 {
            let Ok(actor) = load_pc(&root, race, false, &[], None, None, None) else {
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let loaded = load_npc(&root, 1556).expect("load Huge Hornet dat 1556");
        let Some((bind_lo, bind_hi)) = loaded.bind_pose_bounds(0.0, 1.0) else {
            panic!("Huge Hornet: no bind-pose bounds");
        };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        // The idle hover bobs between two heights; the lowest drawn point across sampled
        // frames is what a static anchor must clear (BakedActor, scene.rs).
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

    fn registered_slot_name(actor: &FfxiRenderActor, slot: usize) -> Option<String> {
        actor.registered_slots[slot].map(|(id, _)| id.as_str().to_string())
    }

    fn slot_frame(actor: &FfxiRenderActor, slot: usize) -> f32 {
        actor.coordinator.animations[slot]
            .as_ref()
            .and_then(|a| a.current_animation.as_ref())
            .map(|c| c.current_frame)
            .unwrap_or(0.0)
    }

    /// The engage flip changes the selection key (use_battle) but a slot whose
    /// clip is base-only keeps that exact clip: it must not restart, its frame
    /// just keeps advancing — the same-clip no-op of research/xim
    /// SkeletonAnimator.kt setNextAnimation, which is what kept the legs in
    /// sync when the top half switched sets.
    #[test]
    fn engage_flip_keeps_the_base_only_slot_phase() {
        let Some(loaded) = load_hume_m() else { return };
        let has_battle_run0 = loaded
            .all_battle_clips()
            .iter()
            .any(|b| b.id.parameterized_match(&DatId::from_str("run0")));
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);

        for _ in 0..2 {
            actor.inputs = inputs_for_pose(PoseState::Run, false);
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        let before = slot_frame(&actor, 0);
        assert!(before > 0.0, "run0 is playing before the engage flip");

        for _ in 0..2 {
            actor.inputs = inputs_for_pose(PoseState::Run, true);
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        let after = slot_frame(&actor, 0);
        if has_battle_run0 {
            // A battle run0 legitimately replaces the base one: the slot restarts.
            assert!(
                after < before,
                "the battle run0 took over: {before} -> {after}"
            );
        } else {
            assert!(
                (after - before - 2.0).abs() < 0.1,
                "the base-only legs slot restarted on the engage flip: {before} -> {after}"
            );
        }
    }

    /// The weapon draw and sheathe play their routine's motion clip exactly once. Driven the way the
    /// live tick drives it — engage machine, then pose step — on the real Hume battle set at the
    /// frame step a 60 fps render takes (half an authored frame per step). Both edges are covered:
    /// while a transition window owns its slots the playhead only ever moves forward, and once the
    /// clip has run out it sits on its last authored frame; the finished layer then hands its slot
    /// back, which is also what gates the next registration.
    #[test]
    fn engage_transition_clips_play_once_and_hold_their_last_frame() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        let step = 0.5_f32;

        let mut previous: [Option<(DatId, f32)>; 8] = [None; 8];
        let mut saw_draw_hold = false;
        let mut saw_sheathe_hold = false;
        // A transition window that never started would leave the walk below asserting nothing.
        let mut saw_drawing = false;
        let mut saw_sheathing = false;
        // A transition clip that outlives the fight is a pose welded onto the actor, so the last
        // step of the script re-checks for one.
        let mut overlay_layer_on_last_step = false;

        let script_len = 4 + 200 + 200;
        let script: Vec<bool> = std::iter::repeat_n(false, 4)
            .chain(std::iter::repeat_n(true, 200))
            .chain(std::iter::repeat_n(false, 200))
            .collect();

        for (index, want) in script.into_iter().enumerate() {
            actor.inputs = inputs_for_pose(PoseState::Idle, want);
            advance_engage(
                &mut actor.engage,
                want,
                &actor.routines,
                &actor.rejected_routines,
                &actor.battle_clips,
                &actor.animations,
                step,
            );
            saw_drawing |= matches!(actor.engage, EngageMachine::Drawing { .. });
            saw_sheathing |= matches!(actor.engage, EngageMachine::Sheathing { .. });

            advance_actor_pose_standalone(&mut actor, step, None);

            let mut overlay_layer = false;
            for (slot, animator) in actor.coordinator.animations.iter().enumerate() {
                let Some(ctx) = animator.as_ref().and_then(|a| a.current_animation.as_ref()) else {
                    previous[slot] = None;
                    continue;
                };
                // Only the transition families are watched: idle and locomotion clips loop by
                // design, and their wrap is not this bug.
                let id = ctx.animation.id;
                let overlay = id.as_str().starts_with("in") || id.as_str().starts_with("ot");
                let (frame, length) = (ctx.current_frame, ctx.animation.length_in_frames());

                if let Some((before_id, before)) = &previous[slot] {
                    if overlay && before_id == &id && frame < before - 0.001 {
                        panic!(
                            "slot {slot} restarted {before_id:?} mid-window: {before:.2} -> {frame:.2}",
                        );
                    }
                }
                if (frame - length).abs() < 0.001 && overlay {
                    if id.as_str().starts_with("in") {
                        saw_draw_hold = true;
                    } else {
                        saw_sheathe_hold = true;
                    }
                }
                previous[slot] = Some((id, frame));
                overlay_layer |= overlay;
            }
            if index + 1 == script_len {
                overlay_layer_on_last_step = overlay_layer;
            }
        }

        assert!(
            !overlay_layer_on_last_step,
            "a draw/sheathe layer is still holding the pose after the fight ended"
        );
        assert!(
            saw_drawing && saw_sheathing,
            "the script never drew or sheathed"
        );
        assert!(saw_draw_hold, "the draw clip never held its last frame");
        assert!(
            saw_sheathe_hold,
            "the sheathe clip never held its last frame"
        );
    }

    /// A transition window that outlasts its own clip must not hand it a second pass. Live, the
    /// crossing comes from dt drift (the render step is 0.49-0.52 authored frames, never exactly
    /// 0.5), and the authored Motion stage for `in 0`/`out0` runs longer than the clip it names
    /// (`dur=72` against a 36-tick clip in the Hume battle set). Either way the window can still be
    /// holding the overlay when the playhead crosses the end, and what plays there is decided by the
    /// loop parameters: without the one-shot the layer wraps to frame 0 and the animation restarts.
    #[test]
    fn an_overlay_window_longer_than_its_clip_does_not_replay_the_clip() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);

        // The window is deliberately set past one pass of the clip; the draw selection follows it.
        actor.inputs = inputs_for_pose(PoseState::Idle, true);
        actor.engage = EngageMachine::Drawing { remaining: 200.0 };

        let mut previous: [Option<(DatId, f32)>; 8] = [None; 8];
        // Real render steps: never the same twice, so the crossing lands mid-window.
        let dts = [0.519_f32, 0.478, 0.506, 0.492, 0.517, 0.483];
        let mut crossed_end = false;

        for dt in dts.iter().cycle().take(220) {
            actor.inputs = inputs_for_pose(PoseState::Idle, true);
            advance_actor_pose_standalone(&mut actor, *dt, None);

            for (slot, animator) in actor.coordinator.animations.iter().enumerate() {
                let Some(ctx) = animator.as_ref().and_then(|a| a.current_animation.as_ref()) else {
                    previous[slot] = None;
                    continue;
                };
                let id = ctx.animation.id;
                if !id.as_str().starts_with("in") {
                    previous[slot] = None;
                    continue;
                }
                let (frame, length) = (ctx.current_frame, ctx.animation.length_in_frames());
                crossed_end |= frame + 0.001 >= length;

                if let Some((before_id, before)) = &previous[slot] {
                    if before_id == &id && frame < before - 0.001 {
                        panic!("slot {slot} restarted {before_id:?}: {before:.2} -> {frame:.2}");
                    }
                }
                previous[slot] = Some((id, frame));
            }
        }

        assert!(crossed_end, "the playhead never reached the clip end");
    }

    /// A slot hands over only once its current layer is finished, so the disengage selection can ask for
    /// the idle clips while its own transition layer is still mid-pass. Recording that refusal as a done
    /// registration ends the conversation, and the finished sheathe pose keeps owning every bone its clip
    /// keys — measured live at the default cap: slots 0/1 pinned on `ota`'s last frame with only idle's
    /// slot-2 clip advancing, until something else changed the selection ~95 s later.
    #[test]
    fn a_declined_idle_registration_is_asked_again_until_it_lands() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        // The engage window and the pose clock drift apart live (and a layer registered underneath an
        // interrupted Action clip starts even later), so widen the gap on purpose: with equal clocks the
        // window expires exactly when the pass ends and nothing is ever refused.
        let dts = [0.519_f32, 0.478, 0.506, 0.492, 0.517, 0.483];
        const WINDOW_OVER_POSE_CLOCK: f32 = 4.0;

        let script: Vec<bool> = std::iter::repeat_n(false, 4)
            .chain(std::iter::repeat_n(true, 60))
            .chain(std::iter::repeat_n(false, 150))
            .collect();
        let script_len = script.len();
        let mut saw_sheathing = false;

        for (index, want) in script.into_iter().enumerate() {
            let dt = dts[index % dts.len()];
            actor.inputs = inputs_for_pose(PoseState::Idle, want);
            advance_engage(
                &mut actor.engage,
                want,
                &actor.routines,
                &actor.rejected_routines,
                &actor.battle_clips,
                &actor.animations,
                dt * WINDOW_OVER_POSE_CLOCK,
            );
            saw_sheathing |= matches!(actor.engage, EngageMachine::Sheathing { .. });
            advance_actor_pose_standalone(&mut actor, dt, None);

            // Whatever a slot holds is what its bookkeeping names; an outstanding request lives in its
            // own list, so this invariant holds even while a handover waits.
            for (slot, animator) in actor.coordinator.animations.iter().enumerate() {
                assert_eq!(
                    registered_slot_name(&actor, slot).as_deref(),
                    animator
                        .as_ref()
                        .and_then(|a| a.current_animation.as_ref())
                        .map(|c| c.animation.id.as_str().to_string())
                        .as_deref(),
                    "slot {slot} at step {index}",
                );
            }

            // The tail of the script is plain idle: no transition layer may still own a slot there.
            if index + 40 >= script_len {
                for (slot, animator) in actor.coordinator.animations.iter().enumerate() {
                    let Some(id) = animator
                        .as_ref()
                        .and_then(|a| a.current_animation.as_ref())
                        .map(|c| c.animation.id)
                    else {
                        continue;
                    };
                    assert!(
                        !(id.as_str().starts_with("in") || id.as_str().starts_with("ot")),
                        "step {index}: slot {slot} still owns the transition clip {}",
                        id.as_str()
                    );
                }
            }
        }

        assert!(saw_sheathing, "the script never sheathed");
    }

    /// A Left/Right movement blend holds exactly the two clips it is blending — idle never enters
    /// it. xim's PoC gave mvl?/mvr? requests a third pose (`inBetween` = idle frame 0,
    /// `research/xim` ActorManager.kt needsInBetweenFrame) as the mitigation for mvl? <-> mvr?
    /// rotating some joints ~180 degrees. `FFXiMain.dll retail-2026-09` gives its blend site (`RVA
    /// 0x33220`, dancer_engine.md §4a-bis) only the outgoing and incoming bone records — one weighted
    /// sum, with the shortest-arc sign flip doing the 180-degree work. Measured on shipped Hume M data the waypoint
    /// dragged held-weapon chains through the casual-idle bone records (`mvl?` registers digit 0/1/2 as
    /// three simultaneous layers and digit 1 owns joint 9 — see gaps §G.12), which is what reads on
    /// screen as the weapon reaching for the other hand on the first strafe frame.
    #[test]
    fn a_movement_blend_holds_only_the_two_clips_it_is_blending() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);

        let animator_on = |actor: &FfxiRenderActor, prefix: &str| {
            (0..8).find_map(|slot| {
                actor.coordinator.animations[slot]
                    .as_ref()
                    .filter(|a| {
                        a.current_animation
                            .as_ref()
                            .is_some_and(|c| c.animation.id.as_str().starts_with(prefix))
                    })
                    .map(|_| slot)
            })
        };

        for _ in 0..2 {
            actor.inputs = inputs_for_pose(PoseState::Run, false);
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        actor.inputs = inputs_for_pose(PoseState::StrafeLeft, false);
        advance_actor_pose_standalone(&mut actor, 1.0, None);

        let Some(slot) = animator_on(&actor, "mvl") else {
            panic!("StrafeLeft must select an mvl? clip on hume");
        };
        let transition = actor.coordinator.animations[slot]
            .as_ref()
            .and_then(|a| a.transition.as_ref())
            .expect("the strafe entry is a fresh register, so its slot transitions");
        assert!(
            transition.next.animation.id.as_str().starts_with("mvl"),
            "the incoming side of the blend must be the requested movement clip"
        );

        let live: Vec<String> = actor
            .coordinator
            .animations
            .iter()
            .filter_map(|a| a.as_ref())
            .filter_map(|a| a.current_animation.as_ref())
            .map(|c| c.animation.id.as_str())
            .collect();
        assert!(
            !live.iter().any(|id| id.starts_with("idl")),
            "no idle clip may be live while strafing — the waypoint put one there: {live:?}"
        );
    }

    /// Locked on and engaged, an A/D side step turns the torso back toward the target through the look-at and no
    /// further than its records allow, and the A/D changeover brings the torso round through the front. The side-step
    /// clips are stood in for by the shape that gives the reported symptom: the lower set turns joint 2 (legs and
    /// upper body both hang off it) 45 degrees, the upper set counter-twists joint 49 100 degrees the other way, so
    /// the legs blend through the front and the torso, blended the shortest way, through the back. Unlocked keeps
    /// exactly that; locked must not. `--nocapture` prints the per-frame yaw table against the body's aim.
    #[test]
    fn a_locked_side_step_turns_the_torso_to_the_target_inside_the_look_at_limits() {
        let Some(loaded) = load_hume_m() else { return };
        let skeleton = &loaded.skeleton;
        let upper =
            crate::locked_torso::UpperBody::of(skeleton).expect("Hume M has chest, neck and feet");
        let reference = |slot: usize| skeleton.reference_at(slot).map(|r| r.index).unwrap();
        let neck = reference(ffxi_dat::skel::standard_position::NECK);
        let foot = reference(ffxi_dat::skel::standard_position::RIGHT_FOOT);
        let chest_limit_deg = skeleton
            .look_at_limits
            .get(1)
            .map(|l| (l.x_limit / l.scale).atan().to_degrees())
            .expect("Hume M authors a chest record");

        let bind = ffxi_actor::skeleton_instance::pose_world(
            skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        let yaw_of = |pose: &[Mat4], joint: usize| {
            let axis = bind[joint].to_scale_rotation_translation().1.inverse() * POSE_FORWARD;
            let f = pose[joint].to_scale_rotation_translation().1 * axis;
            (-f.z).atan2(f.x).to_degrees()
        };

        /// Long enough for `in 0`, the engage draw, to finish and hand the pose to locomotion.
        const ENGAGE_SETTLE_FRAMES: usize = 80;
        const STRAFE_FRAMES: usize = 40;
        /// The tail of each side's strafe, past the crossfade and the lock ramp.
        const SETTLED_FRAMES: usize = 10;
        const PELVIS_TURN_DEG: f32 = 45.0;
        const SPINE_COUNTER_TWIST_DEG: f32 = 100.0;
        const PELVIS: u32 = 2;
        const SPINE: u32 = 49;
        let shipped = |id: &str| {
            loaded
                .battle_clips
                .iter()
                .find(|c| c.id.as_str() == id)
                .unwrap_or_else(|| panic!("Hume M battle set ships {id}"))
                .clone()
        };
        let constant = |deg: f32, frames: usize| {
            let q = Quat::from_rotation_y(deg.to_radians());
            vec![
                ffxi_dat::skel_anim::KeyFrameTransform {
                    rotation: [q.x, q.y, q.z, q.w],
                    translation: [0.0; 3],
                    scale: [1.0; 3],
                };
                frames
            ]
        };
        let stand_in = |id: &str, from: &str, joint: u32, deg: f32| {
            let mut clip = shipped(from);
            clip.id = DatId::from_str(id);
            if id.ends_with('0') {
                clip.key_frame_sets = HashMap::new();
            }
            clip.key_frame_sets
                .insert(joint, constant(deg, clip.num_frames));
            clip
        };
        let stand_ins = vec![
            stand_in("mvl0", "mvl1", PELVIS, PELVIS_TURN_DEG),
            stand_in("mvr0", "mvr1", PELVIS, -PELVIS_TURN_DEG),
            stand_in("mvl1", "mvl1", SPINE, -SPINE_COUNTER_TWIST_DEG),
            stand_in("mvr1", "mvr1", SPINE, SPINE_COUNTER_TWIST_DEG),
        ];

        let run = |locked: bool, middle: &[PoseState]| {
            let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
            let mut battle = stand_ins.clone();
            battle.extend(
                actor
                    .battle_clips
                    .iter()
                    .filter(|c| {
                        !matches!(c.id.as_str().as_str(), "mvl0" | "mvr0" | "mvl1" | "mvr1")
                    })
                    .cloned(),
            );
            actor.battle_clips = Arc::new(battle);
            let mut script: Vec<PoseState> = Vec::new();
            // Engaging plays the weapon draw first, which owns the pose until it ends; the side step starts after it.
            script.extend(std::iter::repeat_n(PoseState::Idle, ENGAGE_SETTLE_FRAMES));
            script.extend(std::iter::repeat_n(PoseState::StrafeLeft, STRAFE_FRAMES));
            script.extend_from_slice(middle);
            script.extend(std::iter::repeat_n(PoseState::StrafeRight, STRAFE_FRAMES));
            let mut rows = Vec::new();
            for st in script {
                actor.inputs = inputs_for_pose(st, true);
                actor.locked_on = locked && actor.inputs.moving;
                advance_engage(
                    &mut actor.engage,
                    true,
                    &actor.routines,
                    &actor.rejected_routines,
                    &actor.battle_clips,
                    &actor.animations,
                    1.0,
                );
                // Engaged, retail's status gate holds the look-at itself off (crate::look_at_gates::look_at_allowed).
                advance_actor_pose(
                    &mut actor,
                    1.0,
                    crate::look_at_gates::LookState::Suppressed,
                    None,
                    None,
                    false,
                    None,
                );
                let pose = actor.world_pose();
                rows.push((
                    st,
                    yaw_of(pose, upper.chest),
                    yaw_of(pose, neck),
                    yaw_of(pose, foot),
                ));
            }
            rows
        };

        let changeover = ENGAGE_SETTLE_FRAMES + STRAFE_FRAMES;
        let left_settled = changeover - SETTLED_FRAMES..changeover;
        for (label, middle) in [
            ("direct", &[][..]),
            ("gap", &[PoseState::Idle, PoseState::Idle][..]),
        ] {
            let free = run(false, middle);
            let held = run(true, middle);
            let right_settled = held.len() - SETTLED_FRAMES..held.len();
            println!("--- changeover {label}: frame state | chest free | chest locked | neck locked | foot");
            let shown = free.iter().zip(held.iter()).enumerate();
            for (i, (free_row, held_row)) in shown.skip(changeover - SETTLED_FRAMES) {
                println!(
                    "{i:>3} {:<7} | {:>7.1} | {:>7.1} | {:>7.1} | {:>7.1}",
                    held_row.0.label(),
                    free_row.1,
                    held_row.1,
                    held_row.2,
                    held_row.3
                );
            }

            if label == "direct" {
                assert!(
                    free.iter().any(|row| row.1.abs() > 90.0),
                    "unlocked, the stand-ins must reproduce the torso swinging through the back"
                );
            }
            assert!(
                free.iter().all(|row| row.3.abs() < 90.0),
                "{label}: the legs come through the front on their own"
            );
            assert!(
                held.iter().all(|row| row.1.abs() < 90.0),
                "{label}: locked, the chest must never pass the back"
            );
            for i in left_settled.clone().chain(right_settled) {
                let (free_chest, held_chest, held_neck) = (free[i].1, held[i].1, held[i].2);
                let turned = (free_chest - held_chest).abs();
                assert!(
                    turned <= chest_limit_deg + 1.0,
                    "{label} frame {i}: the chest turned {turned:.1} deg, past its {chest_limit_deg:.1} deg record"
                );
                assert!(
                    held_chest.abs() + 5.0 < free_chest.abs(),
                    "{label} frame {i}: the look-at must turn the chest toward the target ({free_chest:.1} -> {held_chest:.1})"
                );
                assert!(
                    held_neck.abs() < held_chest.abs(),
                    "{label} frame {i}: the head turns further toward the target than the shoulders"
                );
            }
        }
    }

    /// Strafing engaged must not shrink the upper body. Two mechanisms, both measured on shipped Hume clips:
    ///
    /// * `hum_` joint 89 carries constant-only zero scale channels in the locomotion set (unwritten). Applying
    ///   them literally took its world basis to exactly 0 — joints 90-93 below it with it — the frame `mvl1`/`mvr1`
    ///   owned the bone; idle (`ind?`) does not key that bone, so only strafing collapsed. Parser rule pinned by
    ///   `zero_scale_constants_read_as_identity`.
    /// * A blend stores retail's raw weighted sum (FFXiMain.dll retail-2026-09 RVA 0x33220, no normalisation), and
    ///   a short quaternion in the joint chain scales by |q|²: measured at 0.548 of settled size mid-fade,
    ///   down-chain included. Boundary rule pinned by `a_short_blended_rotation_rotates_without_scaling`.
    ///
    /// Degeneracy is asserted, not a scale band — shipped motion animates scale (-0.033 to 1.31).
    #[test]
    fn strafing_while_engaged_never_collapses_a_bone() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        let step = 0.5_f32;

        // Idle first, then engaged strafing with the bucket flipping every few frames (holding A or D in lock does
        // exactly that to the heading), finishing on a long steady hold so no collapse can hide inside a fade.
        let mut script: Vec<(PoseState, bool)> = Vec::new();
        for _ in 0..8 {
            script.push((PoseState::Idle, false));
        }
        for _ in 0..4 {
            script.push((PoseState::Idle, true));
        }
        let mut phase = PoseState::StrafeLeft;
        for frame in 0..48 {
            if frame % 6 == 0 {
                phase = match phase {
                    PoseState::StrafeLeft => PoseState::StrafeRight,
                    _ => PoseState::StrafeLeft,
                };
            }
            script.push((phase, true));
        }
        for _ in 0..40 {
            script.push((PoseState::StrafeLeft, true));
        }

        let mut best: Vec<f32> = Vec::new();
        let mut saw_strafe_clip = false;
        for (i, (st, eng)) in script.iter().enumerate() {
            actor.inputs = inputs_for_pose(*st, *eng);
            advance_engage(
                &mut actor.engage,
                *eng,
                &actor.routines,
                &actor.rejected_routines,
                &actor.battle_clips,
                &actor.animations,
                step,
            );
            advance_actor_pose(
                &mut actor,
                step,
                crate::look_at_gates::look_state_of(true, false),
                None,
                None,
                false,
                None,
            );

            // These are the clips that key `hum_` joint 89; without one of them on screen this script proves
            // nothing about the collapse.
            if actor.coordinator.animations.iter().flatten().any(|a| {
                a.current_animation.as_ref().is_some_and(|c| {
                    let id = c.animation.id.as_str();
                    id.starts_with("mvl") || id.starts_with("mvr")
                })
            }) {
                saw_strafe_clip = true;
            }

            let scales: Vec<f32> = actor
                .world_pose()
                .iter()
                .map(|m| (m.x_axis.length() + m.y_axis.length() + m.z_axis.length()) / 3.0)
                .collect();
            if best.len() != scales.len() {
                best = vec![0.0; scales.len()];
            }

            for joint in 0..scales.len() {
                let prior_best = best[joint];
                if prior_best > 0.5 && scales[joint] < 1e-4 {
                    panic!(
                        "frame {} ({} engaged={eng}): joint {joint} collapsed to {:.6} after reaching {:.3} in this run",
                        i,
                        st.label(),
                        scales[joint],
                        prior_best,
                    );
                }
                best[joint] = prior_best.max(scales[joint]);
            }
        }
        assert!(
            saw_strafe_clip,
            "no mvl?/mvr? clip ever ran, so this script never exercised the bone that collapsed"
        );
    }

    /// A weapon's Info standard-joint byte is only worth re-parenting if the mesh hangs off the joint it
    /// names; otherwise the override moves an empty subtree and the held mesh still rides the pelvis chain.
    /// Pins the measured byte->joint table, and sweeps every main-hand model this install resolves for Hume M
    /// to check each mesh's bound bones against that handle's ancestor chain. The sweep also pins its own
    /// exception set, so a new escape (or a fix) fails loudly instead of being absorbed.
    #[test]
    fn every_held_weapon_mesh_hangs_off_the_joint_its_info_byte_names() {
        use crate::look_resolver::equipment_dat_id;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");

        const MAIN_HAND_SLOT: u8 = 6;
        /// `(Info standard-joint byte, joint it names in the Hume M skeleton)`, measured over every
        /// main-hand model this install resolves.
        const MEASURED_HANDLES: [(u8, usize); 9] = [
            (0x70, 3),
            (0x71, 4),
            (0x72, 6),
            (0x73, 7),
            (0x74, 9),
            (0x75, 11),
            (0x76, 12),
            (0x77, 14),
            (0x78, 16),
        ];
        /// Highest slot-6 model id resolved on this install (measured); ids above it return nothing.
        const SWEPT_MODEL_IDS: u16 = 24;
        /// Slot-6 model id 18 names joint 12 while its skel_mesh binds bone 15, which hangs off joint 14,
        /// so the re-parent cannot follow that mesh. Pinned as a defect of this install's pairing, not of
        /// the law. Named closer: identify what slot-6 id 18 is here and whether retail pairs it with a
        /// different handle or skeleton.
        const UNMATCHED_MODEL_IDS: [u16; 1] = [18];

        let loaded = load_pc(&root, HUME_M, false, &[], None, None, None).expect("bare Hume M");
        let sk = &loaded.skeleton;
        for (byte, joint) in MEASURED_HANDLES {
            assert_eq!(
                sk.reference_at(usize::from(byte)).map(|r| r.index),
                Some(joint),
                "Info byte {byte:#x} does not name Hume M joint {joint}"
            );
        }
        let ancestors_of = |joint: usize| -> Vec<usize> {
            let mut chain = vec![joint];
            let mut next = sk.joints[joint].parent;
            while let Some(p) = next {
                chain.push(p);
                next = sk.joints[p].parent;
            }
            chain
        };

        let mut swept = 0u16;
        let mut unmatched: Vec<u16> = Vec::new();
        for model_id in 0..=SWEPT_MODEL_IDS {
            let Some(dat) = equipment_dat_id(&dll, MAIN_HAND_SLOT, model_id, HUME_M) else {
                continue;
            };
            let Some(bytes) = read_dat(&root, dat) else {
                continue;
            };
            let dir = ResourceDir::from_bytes(bytes);
            let mut anchors: Vec<usize> = dir
                .collect_skel_meshes()
                .iter()
                .flat_map(|m| m.meshes.iter())
                .flat_map(|b| b.vertices.iter().map(|v| v.joint_index0 as usize))
                .collect();
            anchors.sort_unstable();
            anchors.dedup();
            // Models with no skinned mesh carry the byte but bind nothing (measured ids 0, 19, 20, 21).
            if anchors.is_empty() {
                continue;
            }
            swept += 1;
            let handle = dir
                .first_cib()
                .and_then(|c| c.standard_joint)
                .and_then(|slot| sk.reference_at(usize::from(slot)))
                .map(|r| r.index);
            let outside: Vec<usize> = match handle {
                Some(h) => anchors
                    .iter()
                    .filter(|a| !ancestors_of(**a).contains(&h))
                    .copied()
                    .collect(),
                None => anchors.clone(),
            };
            if !outside.is_empty() {
                unmatched.push(model_id);
                assert!(
                    UNMATCHED_MODEL_IDS.contains(&model_id),
                    "slot-6 model {model_id} binds bone(s) {outside:?} outside its Info handle {handle:?}; \
                     every other swept model's mesh hangs off the joint its own byte names"
                );
            }
        }
        assert!(
            swept >= 20,
            "only {swept} slot-6 models have a skinned mesh, so this sweep proves nothing"
        );
        assert_eq!(
            unmatched, UNMATCHED_MODEL_IDS,
            "the set of main-hand models whose mesh escapes its Info-named handle changed"
        );
    }

    /// Same containment law swept over every playable look race (HumeM=1..Galka=8), because the re-parent is
    /// applied to any PC skeleton and one install-wide pairing is bad in a way this pins. Two facts the sweep
    /// pinned, both measured on this install:
    ///   - Slot-6 model id 18 escapes the law on every race except 7 (Mithra): its mesh binds a bone outside the
    ///     joint its Info byte names, and on Elvaan-male (race 3) that bone is the root. Elvaan-female (race 4)
    ///     additionally fails for model id 1, whose mesh also binds the root.
    ///   - Weapon handles are NOT parented under joint 2 on every rig: races 1,2,4,5,6,7,8 have them there (all
    ///     but the exceptions above), but Elvaan-male (race 3) has zero of its 44 weapon handles under joint 2.
    ///     So "every weapon chain inherits the strafe pelvis swing" is a Hume-class fact, not an engine-wide one.
    #[test]
    fn every_pc_race_hangs_its_weapon_mesh_off_the_joint_its_info_byte_names() {
        use crate::look_resolver::equipment_dat_id;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        const MAIN_HAND_SLOT: u8 = 6;
        /// `(race, model id)` measured on this install where the mesh is bound outside its Info-named handle.
        const MEASURED_ESCAPES: [(u8, u16); 8] = [
            (1, 18),
            (2, 18),
            (3, 18),
            (4, 1),
            (4, 18),
            (5, 18),
            (6, 18),
            (8, 18),
        ];
        /// Highest slot-6 model id any race resolves (measured); ids past it return nothing.
        const SWEPT_MODEL_IDS: u16 = 47;

        let mut swept_total = 0usize;
        for race in 1..=8u8 {
            let loaded =
                load_pc(&root, race, false, &[], None, None, None).expect("bare PC skeleton");
            let sk = &loaded.skeleton;
            let is_under = |mut joint: usize, ancestor: usize| -> bool {
                while joint != ancestor {
                    match sk.joints[joint].parent {
                        Some(p) => joint = p,
                        None => return false,
                    }
                }
                true
            };
            let mut escapes: Vec<u16> = Vec::new();
            for model_id in 0..=SWEPT_MODEL_IDS {
                let Some(dat) = equipment_dat_id(&dll, MAIN_HAND_SLOT, model_id, race) else {
                    continue;
                };
                let Some(bytes) = read_dat(&root, dat) else {
                    continue;
                };
                let dir = ResourceDir::from_bytes(bytes);
                let mut anchors: Vec<usize> = dir
                    .collect_skel_meshes()
                    .iter()
                    .flat_map(|m| m.meshes.iter())
                    .flat_map(|b| b.vertices.iter().map(|v| v.joint_index0 as usize))
                    .collect();
                anchors.sort_unstable();
                anchors.dedup();
                if anchors.is_empty() {
                    continue;
                }
                swept_total += 1;
                let Some(handle) = dir
                    .first_cib()
                    .and_then(|c| c.standard_joint)
                    .and_then(|slot| sk.reference_at(usize::from(slot)))
                    .map(|r| r.index)
                else {
                    escapes.push(model_id);
                    continue;
                };
                if !anchors.iter().all(|a| is_under(*a, handle)) {
                    escapes.push(model_id);
                }
            }
            let mut expected: Vec<u16> = MEASURED_ESCAPES
                .iter()
                .filter(|(r, _)| *r == race)
                .map(|(_, m)| *m)
                .collect();
            expected.sort_unstable();
            assert_eq!(
                escapes, expected,
                "race {race}: main-hand models whose mesh escapes its Info-named handle changed"
            );
        }
        assert!(
            swept_total >= 8 * 40,
            "only {swept_total} weapon meshes swept across all races, so this proves nothing"
        );
    }

    /// A held weapon mesh has no transform of its own: every vertex binds to one skeleton bone, and that
    /// anchor joint authors no bind pose at all (measured zero-transform set on the Hume M rig: 2, 5, 8,
    /// 10, 13, 15, 17, 19, 22, 27). A frame with no keyer on an anchor leaves the held mesh wherever its
    /// parent chain happens to stand, so which motion blocks got loaded decides whether a drawn weapon has
    /// any authored writer. FFXiMain picks those blocks from each hand's own animation-type byte
    /// (research/xim poc/Model.kt PcModel.getMainBattleAnimationDirectory / getSubBattleAnimationDirectory /
    /// getSkirtBattleAnimationResource), which is what [`combat_stance::BattleMotionBlocks::resolve`] ports - of
    /// which only the main-hand stance block may join the body's motion pool, because a second member of an id
    /// family becomes a simultaneous layer rather than a variant.
    /// Pins each main-hand model's anchor joint, how many loaded clips key it on this install, and that the
    /// battle-stance id family has exactly its two shipped members. A pinned count is measured reality, not an
    /// endorsement: dagger model 2 (anchor joint 10) has no writer in any race-wide or stance clip at all, so while
    /// *unengaged* it stands wherever its chain happens to be; while display-engaged the weapon-handle re-parent
    /// supplies its transform instead ([`weapon_handle_overrides`], pinned by
    /// `an_engaged_pc_holds_its_weapon_on_the_hand_not_on_its_own_chain`). A third family member would pose two battle
    /// stances at once and pull the mesh off its hand.
    #[test]
    fn weapon_anchor_writers_follow_the_battle_motion_block_law() {
        use crate::look_resolver::equipment_dat_id;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        const HUME_M: u8 = 1;
        const MAIN_HAND_SLOT: u8 = 6;

        /// One main-hand model id, the joint its mesh is bound to, and how many loaded clips key that joint.
        struct HeldWeapon {
            model_id: u16,
            anchor: usize,
            writers: usize,
        }
        const PINNED: [HeldWeapon; 9] = [
            HeldWeapon {
                model_id: 1,
                anchor: 3,
                writers: 123,
            },
            // The dagger-family anchor has no authored writer in either the race-wide block or its own stance
            // block on this install, so only the engaged handle re-parent gives it a transform.
            HeldWeapon {
                model_id: 2,
                anchor: 10,
                writers: 0,
            },
            HeldWeapon {
                model_id: 4,
                anchor: 5,
                writers: 7,
            },
            HeldWeapon {
                model_id: 5,
                anchor: 5,
                writers: 7,
            },
            HeldWeapon {
                model_id: 9,
                anchor: 6,
                writers: 115,
            },
            HeldWeapon {
                model_id: 10,
                anchor: 15,
                writers: 93,
            },
            HeldWeapon {
                model_id: 11,
                anchor: 13,
                writers: 96,
            },
            HeldWeapon {
                model_id: 12,
                anchor: 13,
                writers: 96,
            },
            HeldWeapon {
                model_id: 13,
                anchor: 8,
                writers: 27,
            },
        ];

        for pinned in PINNED.iter() {
            let Some(main) = equipment_dat_id(&dll, MAIN_HAND_SLOT, pinned.model_id, HUME_M) else {
                continue;
            };
            let mut bound = Vec::new();
            for mesh in
                ResourceDir::from_bytes(read_dat(&root, main).unwrap()).collect_skel_meshes()
            {
                for buffer in &mesh.meshes {
                    for v in &buffer.vertices {
                        let joint = v.joint_index0 as usize;
                        if !bound.contains(&joint) {
                            bound.push(joint);
                        }
                    }
                }
            }
            assert_eq!(
                bound,
                vec![pinned.anchor],
                "model {} binds its mesh to a different joint than measured",
                pinned.model_id
            );

            let equipment = [main]
                .into_iter()
                .chain((1u8..=5).filter_map(|slot| equipment_dat_id(&dll, slot, 0, HUME_M)))
                .collect::<Vec<_>>();
            let loaded = load_pc(&root, HUME_M, false, &equipment, None, Some(main), None)
                .expect("load Hume M with an equipped weapon");

            let keyed = loaded
                .animations
                .iter()
                .chain(loaded.battle_clips.iter())
                .filter(|c| c.key_frame_sets.contains_key(&(pinned.anchor as u32)))
                .count();
            let mut stance: Vec<String> = loaded
                .battle_clips
                .iter()
                .filter(|c| c.id.as_str().starts_with("btl"))
                .map(|c| c.id.as_str().to_string())
                .collect();
            stance.sort();
            assert_eq!(
                keyed, pinned.writers,
                "model {} anchor {}: {} loaded clips key it, pinned {}",
                pinned.model_id, pinned.anchor, keyed, pinned.writers
            );
            // One member set per id family: pose_clip_matches registers every clip matching a parameterized id
            // as a simultaneous layer, so a second `btl?` poses two stances at once.
            assert_eq!(
                stance,
                vec!["btl0".to_string(), "btl1".to_string()],
                "model {} resolved a second battle-stance family member",
                pinned.model_id
            );
        }
    }

    /// The rig carries two parallel attach chains for held weapons, and the last letter/digit of a clip's
    /// id says which one it drives. A dagger (model 2) binds every vertex to joint 10, whose chain is
    /// 2 -> 9 -> 10 with all three authored at zero offset; `btl1`/`mvl1`-style ids key joint 9, while the
    /// matching `btl0`/`mvl0`-style ids key joint 2 and never touch 9. Pinned because it is what makes a
    /// held mesh move at all: nothing in the loaded pool writes joint 10 directly (measured above), so the
    /// weapon follows whichever attach chain the registered clip happens to drive - and registering both
    /// variants of one id family at once puts two motions on two chains, which is a fight over the hand.
    /// Superseded while display-engaged: there the handle joint takes the hand's transform outright
    /// ([`weapon_handle_overrides`]), so keyed-ness of these chains no longer decides where the mesh stands.
    #[test]
    fn held_weapons_follow_an_attach_chain_that_the_clip_id_selects() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        const HUME_M: u8 = 1;
        let dagger =
            crate::look_resolver::equipment_dat_id(&dll, 6, 2, HUME_M).expect("dagger model 2");
        let equipment: Vec<u32> = (1u8..=5)
            .filter_map(|slot| crate::look_resolver::equipment_dat_id(&dll, slot, 0, HUME_M))
            .collect();
        let loaded = load_pc(&root, HUME_M, false, &equipment, None, Some(dagger), None)
            .expect("load Hume M with an equipped dagger");

        let clips = || loaded.animations.iter().chain(loaded.battle_clips.iter());
        let present = |id: &str| clips().any(|c| c.id.as_str() == id);
        let keys = |id: &str, joint: usize| {
            clips()
                .filter(|c| c.id.as_str() == id)
                .any(|c| c.key_frame_sets.contains_key(&(joint as u32)))
        };

        assert_eq!(loaded.skeleton.joints[10].parent, Some(9));
        assert_eq!(loaded.skeleton.joints[9].parent, Some(2));
        // Joint 2 and the anchor itself are authored at zero, so the anchor sits exactly where its parent
        // puts it: the chain contributes one torso-height offset (joint 9) and nothing else. Everything that
        // actually parks a weapon in a hand is motion.
        let near = |j: usize, want: [f32; 3]| {
            let got = loaded.skeleton.joints[j].translation;
            (0..3).all(|k| (got[k] - want[k]).abs() < 1e-4)
        };
        assert!(near(2, [0.0; 3]), "joint 2 authors no offset");
        assert!(
            near(9, [0.0, 1.052848, 0.04654]),
            "joint 9's authored offset is the chain's only reach: {:?}",
            loaded.skeleton.joints[9].translation
        );
        assert!(
            near(10, [0.0; 3]),
            "the weapon anchor itself authors no offset"
        );

        for id in ["btl0", "btl1", "mvl0", "mvl1"] {
            assert!(present(id), "{id} should be loaded for Hume M");
        }
        assert!(
            keys("btl1", 9) && keys("mvl1", 9),
            "digit-1 ids drive the chain that holds the weapon (joint 9)"
        );
        assert!(
            !keys("btl0", 9) && !keys("mvl0", 9),
            "digit-0 ids must not reach joint 9"
        );
        assert!(
            keys("btl0", 2) && keys("mvl0", 2),
            "digit-0 ids drive the other chain (joint 2)"
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let race = chocobo_race_for_colour(kuluu_snapshot::ChocoboColour::Yellow);
        let actor = load_mount_race(&root, race).expect("yellow chocobo race config");
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
        assert!(
            load_mount_race(&root, black).is_ok(),
            "black chocobo race config"
        );
    }

    /// Why `mount_seat_local` reads a chocobo's seat off its spine instead of
    /// the saddle joint every other mount declares: a chocobo race skeleton
    /// leaves that whole per-race block pointing at joint 0 with a zero offset,
    /// so the lookup resolves to the ground and drops the rider through the
    /// floor. Self-skips without a retail install.
    #[test]
    fn chocobo_race_skeletons_define_no_saddle_joints() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let race = chocobo_race_for_colour(kuluu_snapshot::ChocoboColour::Yellow);
        let actor = load_mount_race(&root, race).expect("yellow chocobo race config");
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mount = load_mount_race(
            &root,
            chocobo_race_for_colour(kuluu_snapshot::ChocoboColour::Yellow),
        )
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let has_chi = |mounted: bool| {
            load_pc(&root, 1, mounted, &[], None, None, None)
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
                            stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                            kind: StageKind::Motion,
                            raw_type: 0x05,
                            delay_frames: 0,
                            duration_frames: 0,
                            id: *clip,
                            actor_rotation: None,
                            turn_toward_step_degrees: None,
                            animation_mode: None,
                            max_loops: 0,
                            transition_in: 0,
                            transition_out: 0,
                            random_group: None,
                            sound_range: None,
                            control_flow: None,
                            local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
                            model_transform: None,
                            follow_points: None,
                            screen_color: None,
                            actor_fade: None,
                            idle_transition_time: None,
                            flinch_duration: None,
                            model_visibility: None,
                            spell_effect: None,
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
                            stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                            kind: StageKind::Motion,
                            raw_type: 0x05,
                            delay_frames: 0,
                            duration_frames,
                            id: *clip,
                            actor_rotation: None,
                            turn_toward_step_degrees: None,
                            animation_mode: None,
                            max_loops: 1,
                            transition_in: 0,
                            transition_out: 0,
                            random_group: None,
                            sound_range: None,
                            control_flow: None,
                            local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
                            model_transform: None,
                            follow_points: None,
                            screen_color: None,
                            actor_fade: None,
                            idle_transition_time: None,
                            flinch_duration: None,
                            model_visibility: None,
                            spell_effect: None,
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

    /// `dat-routine-stages <pc skeleton> dead`, half-frames halved, over races 1..=8
    /// (skeletons 7072/10248/13424/16600/19776 shared by both Tarutaru/23176/26352):
    /// the collapse length is authored per race, so it has to be read from the routine.
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

    /// Retail-DAT guard (skips without an install) for the oracle the phase machine is
    /// built on: every PC `dead` routine is a one-shot `ded?` collapse followed by
    /// `cor?`, and `cor?` is itself static -- so what retail holds forever is the corpse
    /// frame, not the collapse.
    #[test]
    fn retail_dead_routine_is_a_one_shot_collapse_into_a_static_corpse_pose() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };

        for (race, expected_frames) in PC_COLLAPSE_FRAMES {
            let actor = load_pc(&root, race, false, &[], None, None, None).expect("load PC race");
            let routines = actor.all_routines();

            let (collapse_id, collapse_frames) =
                death_collapse_clip(&routines).expect("PC dead routine");
            assert_eq!(collapse_id.as_str(), "ded?");
            assert_eq!(
                collapse_frames, expected_frames,
                "race {race} collapse length"
            );
            assert_eq!(
                routine_motion_lookup(
                    &routines,
                    &actor.rejected_routines,
                    actor_state::death_routine_id(),
                    true,
                )
                .ok()
                .flatten()
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

    /// Retail stores `cor?` as compressed keyframes, so its first and last frame decode to the
    /// same pose only up to f32 round-off: measured over all seven PC skeletons the worst gap is
    /// 2.4e-7 on a translation and 2.4e-7 on |q1.q2| - 1.
    const STATIC_POSE_TOLERANCE: f32 = 1e-6;

    fn same_component(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| (x - y).abs() <= STATIC_POSE_TOLERANCE)
    }

    /// A quaternion and its negation name the same orientation, and retail's `cor?` does store
    /// one bone's identity rotation as `[0,0,0,1]` on the first keyframe and `[0,0,0,-1]` on
    /// the last, so the static-pose check compares orientations rather than raw components.
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
        reset_actor_pose_state(&mut actor, 1.0, None);
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

    /// A Raise is a 0 -> >0 transition of that entity's hp_pct on the snapshot: the wire owns
    /// death state again, so tick_live_ffxi_actors clears the Defeated latch that a killing
    /// result started (DeadFromAction) and the pose falls back to idle. Self's raise arrives
    /// through the party row / homepoint timer channel instead of an entity hp_pct. The `dead`
    /// routine is dispatched only by dispatch_melee_action_started on INFO_DEFEATED, so a raise
    /// must not re-fire it: no ActiveScheduler named `dead` may exist after the raise tick.
    #[test]
    fn a_raise_clears_the_defeated_latch_and_returns_to_idle() {
        let Some(install_root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        bevy::tasks::ComputeTaskPool::get_or_init(Default::default);

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

        let tick = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / FRAME_RATE));
            app.update();
        };
        let set_hp = |app: &mut App, id: u32, hp_pct: Option<u8>| {
            for e in &mut app
                .world_mut()
                .resource_mut::<crate::snapshot::SceneState>()
                .snapshot
                .entities
            {
                if e.id == id {
                    e.hp_pct = hp_pct;
                }
            }
        };
        let no_dead_routine = |app: &mut App| {
            // Bevy 0.19: World::query takes &mut self (archetype refresh) and QueryState::iter
            // takes the world separately (zone_point_lights.rs uses the same two-step shape).
            let mut q = app
                .world_mut()
                .query::<&crate::scheduler_runtime::ActiveSchedulers>();
            q.iter(app.world())
                .all(|s| !s.routine_names().any(|n| n == *b"dead"))
        };

        // Mob case: the latch is what dispatch_melee_action_started inserts on a Defeated
        // result (scheduler_runtime.rs); here it is inserted directly and the wire hp_pct owns
        // death state.
        let loaded = load_npc(&install_root, 1568).expect("installed retail NPC DAT"); // Hare
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
            name: Some("Hare".into()),
            pos: kuluu_snapshot::Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            heading: 0,
            hp_pct: Some(0),
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 1,
            char_flags: Default::default(),
            monstrosity: false,
            name_vis: None,
        });
        app.world_mut()
            .entity_mut(actor_entity)
            .insert(crate::scheduler_runtime::DeadFromAction::default());

        tick(&mut app);
        let actor = app.world().get::<FfxiRenderActor>(actor_entity).unwrap();
        assert!(actor.inputs.dead, "the Defeated latch holds the death pose");
        assert!(app
            .world()
            .entity(actor_entity)
            .contains::<crate::scheduler_runtime::DeadFromAction>());

        // Raise: the wire hp_pct goes 0 -> >0. The removal is a command (scheduler_runtime.rs
        // DeadFromAction), so it lands after this frame's pose pass; from the next frame the
        // latch is gone and dead false.
        set_hp(&mut app, 1, Some(50));
        tick(&mut app);
        tick(&mut app);
        assert!(
            !app.world()
                .entity(actor_entity)
                .contains::<crate::scheduler_runtime::DeadFromAction>(),
            "the raise clears the Defeated latch"
        );
        assert!(
            no_dead_routine(&mut app),
            "a raise must not re-fire the dead routine"
        );
        let actor = app.world().get::<FfxiRenderActor>(actor_entity).unwrap();
        assert!(!actor.inputs.dead, "the wire hp_pct owns death state again");

        // Idle within the death-clip length: with no collapse clip to play (or once it has run),
        // the pose resolves back to the idle family (ffxi-actor/src/actor_state.rs
        // idle_animation_id).
        let bound = death_collapse_clip(&actor.routines)
            .map_or(1.0, |(_, f)| f.max(1.0))
            .ceil() as usize
            + 2;
        for _ in 0..bound {
            tick(&mut app);
        }
        let actor = app.world().get::<FfxiRenderActor>(actor_entity).unwrap();
        assert!(
            actor
                .last_clip
                .is_some_and(|c| c.parameterized_match(&DatId::from_str("idl?"))),
            "the raised mob returns to idle within the death-clip length"
        );

        // Self case: self's entity hp_pct stays 100 (it only updates when CHAR_PC carries
        // UPDATE_HP), so death and raise both arrive through the party row / homepoint timer
        // channel that self_dead reads (snapshot.rs resolve_self).
        let loaded = load_pc(&install_root, 1, false, &[], None, None, None)
            .expect("installed retail PC DAT");
        let skin = app
            .world_mut()
            .resource_mut::<FfxiSkinRegistry>()
            .alloc_skin();
        let self_entity = app
            .world_mut()
            .spawn((
                make_render_actor(&loaded, skin, Vec::new(), 42, 0.0, 1.0),
                GlobalTransform::default(),
                Visibility::Inherited,
            ))
            .id();
        let snapshot = &mut app
            .world_mut()
            .resource_mut::<crate::snapshot::SceneState>()
            .snapshot;
        snapshot.self_char_id = Some(42);
        snapshot.entities.push(kuluu_snapshot::Entity {
            id: 42,
            act_index: 42,
            kind: kuluu_snapshot::EntityKind::Pc,
            name: Some("Self".into()),
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
            animationsub: 0,
            mount: None,
            status: 1,
            char_flags: Default::default(),
            monstrosity: false,
            name_vis: None,
        });
        snapshot.party.push(kuluu_snapshot::PartyMember {
            id: 42,
            act_index: 42,
            name: Some("Self".into()),
            hp: 0,
            mp: 0,
            tp: 0,
            hp_pct: 0,
            mp_pct: 0,
            zone_no: 103,
            main_job: 1,
            main_job_lv: 1,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        });
        snapshot.death_homepoint_secs = Some(30);
        app.world_mut()
            .entity_mut(self_entity)
            .insert(crate::scheduler_runtime::DeadFromAction::default());

        tick(&mut app);
        let actor = app.world().get::<FfxiRenderActor>(self_entity).unwrap();
        assert!(
            actor.inputs.dead,
            "the party row / homepoint channel holds self's death pose"
        );

        // Raise: the party row recovers and the homepoint timer clears (snapshot.rs party).
        for m in &mut app
            .world_mut()
            .resource_mut::<crate::snapshot::SceneState>()
            .snapshot
            .party
        {
            if m.id == 42 {
                m.hp = 500;
                m.hp_pct = 50;
            }
        }
        app.world_mut()
            .resource_mut::<crate::snapshot::SceneState>()
            .snapshot
            .death_homepoint_secs = None;
        tick(&mut app);
        tick(&mut app);
        assert!(
            !app.world()
                .entity(self_entity)
                .contains::<crate::scheduler_runtime::DeadFromAction>(),
            "the raise clears self's Defeated latch"
        );
        assert!(
            no_dead_routine(&mut app),
            "a raise must not re-fire the dead routine for self"
        );
        let actor = app.world().get::<FfxiRenderActor>(self_entity).unwrap();
        assert!(
            !actor.inputs.dead,
            "the party row / homepoint channel owns self's death state"
        );

        let bound = death_collapse_clip(&actor.routines)
            .map_or(1.0, |(_, f)| f.max(1.0))
            .ceil() as usize
            + 2;
        for _ in 0..bound {
            tick(&mut app);
        }
        let actor = app.world().get::<FfxiRenderActor>(self_entity).unwrap();
        assert!(
            actor
                .last_clip
                .is_some_and(|c| c.parameterized_match(&DatId::from_str("idl?"))),
            "the raised self returns to idle within the death-clip length"
        );
    }

    #[test]
    fn routine_motion_clip_resolves_first_motion_stage() {
        let routines = synth_routines(&[(b"ati0", b"at0?"), (b"in 0", b"ind?")]);
        assert_eq!(
            routine_motion_clip(&routines, &[], DatId::from_str("ati0")).map(|d| d.as_str()),
            Some("at0?".to_string())
        );

        assert_eq!(
            routine_motion_clip(&routines, &[], DatId::from_str("in 0")).map(|d| d.as_str()),
            Some("ind?".to_string())
        );

        assert_eq!(
            routine_motion_clip(&routines, &[], DatId::from_str("cawh")),
            None
        );
    }

    #[test]
    fn settle_motion_clip_is_the_stage_after_the_wind_up() {
        let mut routines = synth_routines(&[(b"cait", b"mi0?"), (b"cast", b"mb0?")]);
        let wind_up = routine_motion_clip(&routines, &[], DatId::from_str("cait")).unwrap();
        assert_eq!(
            settle_motion_clip(&routines, &[], DatId::from_str("cait"), wind_up),
            None,
            "a routine with one Motion stage has nothing to hand off to"
        );

        let stage = routines[&DatId::from_str("cait")].stages[0];
        routines
            .get_mut(&DatId::from_str("cait"))
            .unwrap()
            .stages
            .push(ffxi_dat::scheduler::TimedStage {
                stage: ffxi_dat::scheduler::SchedulerStage {
                    id: *b"mi1?",
                    ..stage.stage
                },
                ..stage
            });
        assert_eq!(
            settle_motion_clip(&routines, &[], DatId::from_str("cait"), wind_up)
                .map(|d| d.as_str()),
            Some("mi1?".to_string())
        );

        let cast = routine_motion_clip(&routines, &[], DatId::from_str("cast")).unwrap();
        assert_eq!(
            settle_motion_clip(&routines, &[], DatId::from_str("cast"), cast),
            None,
            "the magic cast pose loops its single stage; it never settles elsewhere"
        );
    }

    // Retail-DAT guard (skips without an install). An item use has an activation window the
    // server times (vendor/server/src/map/ai/states/item_state.cpp CItemState::Update reads
    // `item_usable.activation`), and `cait` is authored to cover it: a short wind-up plus a
    // settled stage several times its length. Stopping at the wind-up drops the character back
    // to idle while the use is still running.
    #[test]
    fn an_item_use_holds_its_settled_stage_past_the_wind_up() {
        let Some(loaded) = load_hume_m() else {
            return;
        };
        let routines = loaded.all_routines();
        let routine = DatId::from_str("cait");
        let wind_up = routine_motion_clip(&routines, &[], routine).expect("HumeM cait wind-up");
        let settled =
            settle_motion_clip(&routines, &[], routine, wind_up).expect("HumeM cait settles");

        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        let len = rest_clip_len_frames(&actor.battle_clips, wind_up)
            .max(rest_clip_len_frames(&actor.animations, wind_up));
        assert!(len > 0.0, "HumeM ships the wind-up clip");
        actor.action = Some(ActionPlayback {
            clip_id: wind_up,
            looping: false,
            remaining: len,
            num_loops: None,
            transition_in: LOCOMOTION_XFADE_IN,
            transition_out: LOCOMOTION_XFADE_OUT,
            cast_pose: false,
            settle: Some(settled),
        });

        advance_actor_pose_standalone(&mut actor, 1.0, None);
        assert!(
            actor
                .last_clip
                .is_some_and(|c| c.parameterized_match(&wind_up)),
            "the wind-up plays first"
        );

        // Well past the wind-up's own length, and past anything the activation window could be
        // (item_state.cpp CItemState::Update).
        for _ in 0..(len as usize * 8) {
            advance_actor_pose_standalone(&mut actor, 1.0, None);
        }
        assert!(
            actor
                .last_clip
                .is_some_and(|c| c.parameterized_match(&settled)),
            "the settled stage owns the pose until the use resolves"
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

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(HUME_M_SKELETON_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let (schedulers, _, _) = crate::scheduler_runtime::parse_action_bytes(&bytes);
        let routines: HashMap<DatId, Scheduler> = schedulers
            .into_iter()
            .map(|s| (DatId::from_name(&s.name), s))
            .collect();
        assert_eq!(
            routine_motion_clip(&routines, &[], routine).map(|d| d.as_str()),
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
        use crate::look_resolver::equipment_dat_id;
        const HUME_M: u8 = 1;
        const MAIN_HAND_SLOT: u8 = 6;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        let main_weapon =
            equipment_dat_id(&dll, MAIN_HAND_SLOT, 0, HUME_M).expect("HumeM main-hand model 0");
        let mut equipment = vec![main_weapon];
        equipment.extend((1u8..=5).filter_map(|slot| equipment_dat_id(&dll, slot, 0, HUME_M)));
        let actor = load_pc(
            &root,
            HUME_M,
            false,
            &equipment,
            None,
            Some(main_weapon),
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

        // RangedFinish plays the actor's own shot motion, one-shot
        // (research/xim Actor.kt onRangedAttack).
        assert_eq!(
            r(2, 0, None, None),
            Some(("shlg".to_string(), false)),
            "the ranged finish plays the shot"
        );

        for finish in [3u8, 4, 5, 6, 0] {
            assert_eq!(
                r(finish, 0, None, None),
                None,
                "category {finish} should not pose"
            );
        }
    }

    // An interrupted aim re-issues the ranged-start category carrying the
    // SkillInterrupt animation (vendor/server/src/map/action/interrupts.cpp
    // RangedInterrupt): the looping aim pose must drop, while a plain start
    // still arms it.
    #[test]
    fn ranged_interrupt_drops_the_aiming_pose() {
        use bevy::ecs::system::RunSystemOnce;

        const CALG: u32 = 0x676C6163;

        let mut world = World::new();
        world.init_resource::<crate::snapshot::EventLog>();
        world.init_resource::<SpellSuffixCache>();
        world.init_resource::<crate::scheduler_runtime::MeleeTravel>();
        world.init_resource::<ActorDatRoot>();
        world.init_resource::<crate::combat_stance::EntityMotion>();
        let skeleton = Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        };
        let mut actor = render_actor_for_test(skeleton, vec![Mat4::IDENTITY]);
        actor.world_id = 7;
        actor.routines = Arc::new(synth_routines(&[(b"calg", b"cl0?")]));
        let ent = world.spawn(actor).id();

        let run_overlay = |world: &mut World| {
            world
                .run_system_once(
                    |events: Res<crate::snapshot::EventLog>,
                     motion: Res<crate::combat_stance::EntityMotion>,
                     travel_memo: ResMut<crate::scheduler_runtime::MeleeTravel>,
                     q: Query<&mut FfxiRenderActor>,
                     last: Local<u64>,
                     suffix: ResMut<SpellSuffixCache>,
                     root: Res<ActorDatRoot>| {
                        dispatch_action_overlay(events, motion, travel_memo, q, last, suffix, root)
                    },
                )
                .unwrap();
        };

        let ranged = |animation: Option<u16>| kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id: 7,
            action_id: CALG,
            action_kind: ffxi_proto::melee::CATEGORY_RANGED_START,
            target_id: None,
            result: None,
            animation,
            outcome: None,
        };

        // The aim start arms the looping pose.
        world
            .resource_mut::<crate::snapshot::EventLog>()
            .push(ranged(None));
        run_overlay(&mut world);
        {
            let mut em = world.entity_mut(ent);
            let actor = em.get_mut::<FfxiRenderActor>().unwrap();
            assert!(
                actor.action.is_some_and(|a| a.looping),
                "a ranged start arms the looping aim pose"
            );
        }

        // The interrupt (start category + SkillInterrupt animation) drops it.
        world
            .resource_mut::<crate::snapshot::EventLog>()
            .push(ranged(Some(ffxi_proto::melee::RANGED_INTERRUPT_ANIMATION)));
        run_overlay(&mut world);
        {
            let mut em = world.entity_mut(ent);
            let actor = em.get_mut::<FfxiRenderActor>().unwrap();
            assert!(actor.action.is_none(), "the interrupt drops the aim pose");
        }
    }

    // A mob DAT that lacks the spell school's cast routine (cawh & co) still
    // shows the start: the pose falls back to the generic `cast` routine.
    #[test]
    fn a_mob_magic_start_without_the_school_routine_falls_back_to_cast() {
        use bevy::ecs::system::RunSystemOnce;

        const CAWH: u32 = 0x68776163;

        let mut world = World::new();
        world.init_resource::<crate::snapshot::EventLog>();
        world.init_resource::<SpellSuffixCache>();
        world.init_resource::<crate::scheduler_runtime::MeleeTravel>();
        world.init_resource::<ActorDatRoot>();
        world.init_resource::<crate::combat_stance::EntityMotion>();
        let skeleton = Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        };
        let mut actor = render_actor_for_test(skeleton, vec![Mat4::IDENTITY]);
        actor.world_id = 7;
        actor.routines = Arc::new(synth_routines(&[(b"cast", b"cl0?")]));
        let ent = world.spawn(actor).id();

        let run_overlay = |world: &mut World| {
            world
                .run_system_once(
                    |events: Res<crate::snapshot::EventLog>,
                     motion: Res<crate::combat_stance::EntityMotion>,
                     travel_memo: ResMut<crate::scheduler_runtime::MeleeTravel>,
                     q: Query<&mut FfxiRenderActor>,
                     last: Local<u64>,
                     suffix: ResMut<SpellSuffixCache>,
                     root: Res<ActorDatRoot>| {
                        dispatch_action_overlay(events, motion, travel_memo, q, last, suffix, root)
                    },
                )
                .unwrap();
        };

        let magic_start = kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id: 7,
            action_id: CAWH,
            action_kind: MAGIC_START_CATEGORY,
            target_id: None,
            result: None,
            animation: None,
            outcome: None,
        };

        world
            .resource_mut::<crate::snapshot::EventLog>()
            .push(magic_start);
        run_overlay(&mut world);
        {
            let mut em = world.entity_mut(ent);
            let actor = em.get_mut::<FfxiRenderActor>().unwrap();
            assert!(
                actor.action.is_some_and(|a| a.looping && a.cast_pose),
                "a magic start without the school routine holds the cast pose"
            );
        }
    }

    // The pose pass must answer a travelling attacker with the routine the scheduler armed: research/xim
    // Actor.kt onAttackMainHand picks `atf0/atl0/atr0/atb0` by direction of travel, so if only
    // `dispatch_melee_action_started` moved, the swing's sound and damage chain run while the body is
    // still holding the standing clip. Each synth routine names its own clip so the assertion can tell
    // which one resolved; the real race weapon DAT shares a lunge between `atf0` and `atb0`
    // (`dat-routine-stages 9672 at`).
    #[test]
    fn a_travelling_swing_poses_its_own_clip() {
        use bevy::ecs::system::RunSystemOnce;

        let pose_for = |routines: &[(&[u8; 4], &[u8; 4])], travel: (f32, f32), victim_x: f32| {
            let mut world = World::new();
            world.init_resource::<crate::snapshot::EventLog>();
            world.init_resource::<SpellSuffixCache>();
            world.init_resource::<crate::scheduler_runtime::MeleeTravel>();
            world.init_resource::<ActorDatRoot>();

            let mut motion = crate::combat_stance::EntityMotion::default();
            motion.by_id.insert(
                1,
                crate::combat_stance::MotionSample {
                    smooth_vx: travel.0,
                    smooth_vz: travel.1,
                    ..Default::default()
                },
            );
            motion.by_id.insert(
                2,
                crate::combat_stance::MotionSample {
                    last_pos: Vec3::new(victim_x, 0.0, 0.0),
                    ..Default::default()
                },
            );
            world.insert_resource(motion);

            let skeleton = Skeleton {
                id: DatId::from_str("test"),
                joints: Vec::new(),
                references: Vec::new(),
                bounding_boxes: Vec::new(),
                look_at_limits: Vec::new(),
            };
            let mut actor = render_actor_for_test(skeleton, vec![Mat4::IDENTITY]);
            actor.world_id = 1;
            actor.routines = Arc::new(synth_routines(routines));
            let ent = world.spawn(actor).id();

            world.resource_mut::<crate::snapshot::EventLog>().push(
                kuluu_snapshot::ViewerEvent::ActionStarted {
                    actor_id: 1,
                    action_id: 0,
                    action_kind: ffxi_proto::melee::CATEGORY_BASIC_ATTACK,
                    target_id: Some(2),
                    result: None,
                    animation: Some(ffxi_proto::melee::AttackAnimation::RightAttack.to_wire()),
                    outcome: None,
                },
            );
            world
                .run_system_once(
                    |events: Res<crate::snapshot::EventLog>,
                     motion: Res<crate::combat_stance::EntityMotion>,
                     travel_memo: ResMut<crate::scheduler_runtime::MeleeTravel>,
                     q: Query<&mut FfxiRenderActor>,
                     last: Local<u64>,
                     suffix: ResMut<SpellSuffixCache>,
                     root: Res<ActorDatRoot>| {
                        dispatch_action_overlay(events, motion, travel_memo, q, last, suffix, root)
                    },
                )
                .unwrap();
            world
                .entity(ent)
                .get::<FfxiRenderActor>()
                .and_then(|a| a.action.map(|p| p.clip_id.as_str().to_string()))
        };

        const STANDING_AND_RIGHT: &[(&[u8; 4], &[u8; 4])] =
            &[(b"ati0", b"at0?"), (b"atr0", b"amr?")];

        // +Z travel with the target at +X is travel to its right: xim's lateral axis, bearing x UP.
        assert_eq!(
            pose_for(STANDING_AND_RIGHT, (0.0, 1.0), 2.0).as_deref(),
            Some("amr?"),
            "a circling attacker poses the strafe-attack clip"
        );

        assert_eq!(
            pose_for(&[(b"ati0", b"at0?"), (b"atf0", b"amb?")], (1.0, 0.0), 2.0).as_deref(),
            Some("amb?"),
            "an attacker closing on its target poses the advancing clip"
        );

        assert_eq!(
            pose_for(STANDING_AND_RIGHT, (1.0, 0.0), 2.0).as_deref(),
            Some("at0?"),
            "an uncarried direction falls back to the standing limb clip"
        );
        assert_eq!(
            pose_for(STANDING_AND_RIGHT, (0.0, 0.0), 2.0).as_deref(),
            Some("at0?"),
            "a standing attack keeps the standing limb clip"
        );
    }

    // The same resolution over shipped data instead of synth stubs: these are the clip ids the hume_m
    // weapon-motion DAT really carries, so a rewire that quietly keeps asking `at0?` fails here too.
    #[test]
    fn a_travelling_swing_resolves_the_shipped_directional_clip() {
        use bevy::ecs::system::RunSystemOnce;

        const HUME_M: u8 = 1;
        const MAIN_HAND_SLOT: u8 = 6;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        let main_weapon = crate::look_resolver::equipment_dat_id(&dll, MAIN_HAND_SLOT, 0, HUME_M)
            .expect("Hume M main-hand model 0");
        let loaded = load_pc(
            &root,
            HUME_M,
            false,
            &[main_weapon],
            None,
            Some(main_weapon),
            None,
        )
        .expect("load Hume M with a main-hand weapon");
        let pose_with_travel = |travel: (f32, f32)| -> Option<DatId> {
            let mut world = World::new();
            world.init_resource::<crate::snapshot::EventLog>();
            world.init_resource::<SpellSuffixCache>();
            world.init_resource::<crate::scheduler_runtime::MeleeTravel>();
            world.init_resource::<ActorDatRoot>();
            let mut motion = crate::combat_stance::EntityMotion::default();
            motion.by_id.insert(
                1,
                crate::combat_stance::MotionSample {
                    smooth_vx: travel.0,
                    smooth_vz: travel.1,
                    ..Default::default()
                },
            );
            motion.by_id.insert(
                2,
                crate::combat_stance::MotionSample {
                    last_pos: Vec3::new(2.0, 0.0, 0.0),
                    ..Default::default()
                },
            );
            world.insert_resource(motion);
            let actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
            let ent = world.spawn(actor).id();
            world.resource_mut::<crate::snapshot::EventLog>().push(
                kuluu_snapshot::ViewerEvent::ActionStarted {
                    actor_id: 1,
                    action_id: 0,
                    action_kind: ffxi_proto::melee::CATEGORY_BASIC_ATTACK,
                    target_id: Some(2),
                    result: None,
                    animation: Some(ffxi_proto::melee::AttackAnimation::RightAttack.to_wire()),
                    outcome: None,
                },
            );
            world
                .run_system_once(
                    |events: Res<crate::snapshot::EventLog>,
                     motion: Res<crate::combat_stance::EntityMotion>,
                     travel_memo: ResMut<crate::scheduler_runtime::MeleeTravel>,
                     q: Query<&mut FfxiRenderActor>,
                     last: Local<u64>,
                     suffix: ResMut<SpellSuffixCache>,
                     root: Res<ActorDatRoot>| {
                        dispatch_action_overlay(events, motion, travel_memo, q, last, suffix, root)
                    },
                )
                .unwrap();
            world
                .entity(ent)
                .get::<FfxiRenderActor>()
                .and_then(|a| a.action.map(|p| p.clip_id))
        };

        // Which clip each swing names lives in that weapon-animation-type's own DAT: this model
        // resolves `atf0` to `amf?`, while another Hume M motion DAT routes the same routine name to
        // `amb?`. So the mapping is read from the data here rather than quoted - the routine *names*
        // are universal, the clips are not.
        let motion_of = |routine: &str| -> Option<DatId> {
            loaded
                .routines
                .get(&DatId::from_str(routine))
                .and_then(|s| s.stages.iter().find(|t| t.stage.kind == StageKind::Motion))
                .map(|t| DatId(t.stage.id))
        };
        for (routine, clip) in [
            ("ati0", "at0?"),
            ("atf0", "amf?"),
            ("atb0", "amb?"),
            ("atr0", "amr?"),
            ("atl0", "aml?"),
        ] {
            let named =
                motion_of(routine).unwrap_or_else(|| panic!("{routine} has no Motion stage"));
            assert!(
                named.parameterized_match(&DatId::from_str(clip)),
                "{routine} names {}, expected {clip}",
                named.as_str()
            );
        }

        // And the pose pass asks for exactly those: +Z travel with the victim at +X is travel to its
        // right, -Z to its left; a stationary attacker keeps the limb routine.
        let resolve = |travel: (f32, f32), wildcard: &str| {
            let clip =
                pose_with_travel(travel).unwrap_or_else(|| panic!("{wildcard} posed nothing"));
            assert!(
                clip.parameterized_match(&DatId::from_str(wildcard)),
                "travel {travel:?} resolved {}, expected {wildcard}",
                clip.as_str()
            );
        };
        resolve((1.0, 0.0), "amf?");
        resolve((0.0, 1.0), "amr?");
        resolve((-1.0, 0.0), "amb?");
        resolve((0.0, -1.0), "aml?");
        resolve((0.0, 0.0), "at0?");
    }

    #[test]
    fn engage_machine_draws_then_sheathes() {
        use actor_state::EngageAnimationState as S;
        let routines = synth_routines(&[(b"in 0", b"ind?"), (b"out0", b"otd?")]);

        let anims = vec![synth_anim(b"ind0", 2), synth_anim(b"otd0", 1)];
        let mut m = EngageMachine::NotEngaged;
        let step =
            |m: &mut EngageMachine, want| advance_engage(m, want, &routines, &[], &anims, &[], 1.0);

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
            advance_engage(&mut m, true, &routines, &[], &anims, &[], 1.0),
            S::Engaged
        );
        assert_eq!(
            advance_engage(&mut m, false, &routines, &[], &anims, &[], 1.0),
            S::NotEngaged
        );
    }

    /// A draw clip that ships only in the base animation set still opens the
    /// Drawing window; the battle set alone is not the lookup.
    #[test]
    fn engage_machine_sizes_the_window_from_the_base_set_too() {
        use actor_state::EngageAnimationState as S;
        let routines = synth_routines(&[(b"in 0", b"ind?"), (b"out0", b"otd?")]);
        let battle: Vec<SkeletonAnimation> = Vec::new();
        let base = vec![synth_anim(b"ind0", 2), synth_anim(b"otd0", 1)];
        let mut m = EngageMachine::NotEngaged;
        assert_eq!(
            advance_engage(&mut m, true, &routines, &[], &battle, &base, 1.0),
            S::Engaging
        );
        assert!(matches!(m, EngageMachine::Drawing { .. }));
        assert_eq!(
            advance_engage(&mut m, true, &routines, &[], &battle, &base, 1.0),
            S::Engaging
        );
        assert_eq!(
            advance_engage(&mut m, true, &routines, &[], &battle, &base, 1.0),
            S::Engaged
        );
        assert_eq!(
            advance_engage(&mut m, false, &routines, &[], &battle, &base, 1.0),
            S::Disengaging
        );
        assert!(matches!(m, EngageMachine::Sheathing { .. }));
    }

    /// When both sets carry the draw clip, the window is the battle set's
    /// (the one that plays), not the longest variant anywhere.
    #[test]
    fn engage_machine_window_is_the_battle_clip_when_both_sets_have_it() {
        use actor_state::EngageAnimationState as S;
        let routines = synth_routines(&[(b"in 0", b"ind?"), (b"out0", b"otd?")]);
        let battle = vec![synth_anim(b"ind0", 2), synth_anim(b"otd0", 1)];
        let base = vec![synth_anim(b"ind3", 20), synth_anim(b"otd3", 20)];
        let mut m = EngageMachine::NotEngaged;
        assert_eq!(
            advance_engage(&mut m, true, &routines, &[], &battle, &base, 1.0),
            S::Engaging
        );
        assert!(
            matches!(m, EngageMachine::Drawing { remaining } if (remaining - 2.0).abs() < f32::EPSILON),
            "the window must be the battle clip's 2 frames, got {m:?}"
        );
        assert_eq!(
            advance_engage(&mut m, true, &routines, &[], &battle, &base, 1.0),
            S::Engaging
        );
        assert_eq!(
            advance_engage(&mut m, true, &routines, &[], &battle, &base, 1.0),
            S::Engaged
        );
        assert_eq!(
            advance_engage(&mut m, false, &routines, &[], &battle, &base, 1.0),
            S::Disengaging
        );
        assert!(
            matches!(m, EngageMachine::Sheathing { remaining } if (remaining - 1.0).abs() < f32::EPSILON),
            "the sheathe window must be the battle clip's 1 frame, got {m:?}"
        );
    }

    /// The self pose reads the wire motion only under the goals where the
    /// reactor moves the player; an engage goal, pending or accepted, leaves
    /// the pose on the keys.
    #[test]
    fn self_pose_follows_reactor_only_under_movement_goals() {
        use kuluu_snapshot::ReactorGoal as G;
        let follows = [
            G::Following {
                target_id: 7,
                distance: 2.0,
            },
            G::Pathing {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                waypoints_remaining: 1,
            },
            G::Banking {
                threshold: 50,
                mog_house_zoneline: 1,
            },
        ];
        for g in &follows {
            assert!(self_pose_follows_reactor(Some(g)), "{g:?}");
        }
        let keys = [
            G::Idle,
            G::Engaging {
                target_id: 7,
                attack_issued: true,
            },
            G::Engaged {
                target_id: 7,
                attack_issued: true,
            },
        ];
        for g in &keys {
            assert!(!self_pose_follows_reactor(Some(g)), "{g:?}");
        }
        assert!(!self_pose_follows_reactor(None));
    }

    /// KnockBackInstance: the knock-down clip goes up at once with the run's
    /// lock, each step shoves dir * elapsed * (level / 2) / 8, the stand-up
    /// clip starts once the knock-down's frames are spent, and the run ends
    /// 8 frames after that.
    #[test]
    fn knockback_shoves_then_stands_up() {
        let skeleton = Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        };
        let mut actor = render_actor_for_test(skeleton, Vec::new());
        assert!(actor.advance_knockback(1.0).is_none());

        actor.begin_knockback(Vec2::new(1.0, 0.0), 4, 10.0);
        assert!(actor.knockback_active());
        assert!(
            actor
                .action
                .is_some_and(|a| a.clip_id == DatId::from_str(KNOCKBACK_DOWN_CLIP)),
            "the knock-down clip must be up at once"
        );

        let step = actor.advance_knockback(2.0).expect("running");
        assert!(
            (step.x - 0.5).abs() < 1e-6,
            "2 frames * (4 / 2) / 8 = 0.5, got {step:?}"
        );
        assert!(step.y.abs() < 1e-6);
        assert!(actor.knockback_active());
        assert!(actor
            .action
            .is_some_and(|a| a.clip_id == DatId::from_str(KNOCKBACK_DOWN_CLIP)));

        let _ = actor.advance_knockback(9.0);
        assert!(
            actor
                .action
                .is_some_and(|a| a.clip_id == DatId::from_str(KNOCKBACK_STAND_UP_CLIP)),
            "past the knock-down's 10 frames the stand-up clip takes over"
        );
        assert!(
            actor.knockback_active(),
            "the lock holds through the stand-up"
        );

        let last = actor
            .advance_knockback(7.0)
            .expect("still running for the stand-up");
        assert!(last.x > 0.0, "the shove keeps going during the stand-up");
        assert!(!actor.knockback_active(), "18 frames in, the run is over");
        assert!(actor.advance_knockback(1.0).is_none());
    }

    #[test]
    fn engage_transition_in_progress_only_while_drawing_or_sheathing() {
        let skeleton = Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        };
        let cases = [
            (EngageMachine::NotEngaged, false),
            (EngageMachine::Drawing { remaining: 0.5 }, true),
            (EngageMachine::Engaged, false),
            (EngageMachine::Sheathing { remaining: 0.5 }, true),
        ];
        for (engage, in_progress) in cases {
            let mut actor = render_actor_for_test(skeleton.clone(), Vec::new());
            actor.engage = engage;
            assert_eq!(actor.engage_transition_in_progress(), in_progress);
        }
    }

    #[test]
    fn weapon_never_out_without_an_active_target() {
        use ffxi_proto::decode::animation::ATTACK;
        assert!(
            !weapon_drawn(ATTACK, false, false),
            "engaged, target gone: sheathes"
        );
        assert!(
            weapon_drawn(ATTACK, true, false),
            "engaged, target live: out"
        );
        assert!(
            !weapon_drawn(0, true, false),
            "not engaged: sheathed with a target"
        );
        assert!(
            !weapon_drawn(0, false, false),
            "not engaged: sheathed without a target"
        );
    }

    #[test]
    fn animation_locked_status_holds_the_weapon_past_the_corpse() {
        use ffxi_proto::decode::animation::ATTACK;
        assert!(
            weapon_drawn(ATTACK, false, true),
            "stone-locked: stays drawn past the corpse"
        );
        assert!(
            !weapon_drawn(ATTACK, false, false),
            "no lock, no target: sheathed"
        );
        assert!(
            weapon_drawn(ATTACK, true, true),
            "lock with a live target: still out"
        );
    }

    #[test]
    fn has_animation_lock_effect_matches_the_prevent_action_set() {
        // Each of the server's HasPreventActionEffect icons locks
        // (vendor/server/src/map/status_effect_container.cpp).
        for icon in [2u16, 7, 10, 14, 17, 19, 28, 159, 193] {
            assert!(has_animation_lock_effect(&[icon]), "icon {icon} must lock");
        }
        assert!(
            !has_animation_lock_effect(&[40, 41]),
            "Protect/Shell do not lock"
        );
        assert!(!has_animation_lock_effect(&[]));
        assert!(
            has_animation_lock_effect(&[40, 7, 41]),
            "a lock among non-locks still detects"
        );
    }

    #[test]
    fn real_routines_resolve_to_clips() {
        let Some(actor) = load_hume_m() else { return };
        let routines = actor.all_routines();
        let clip = |routine: &str| {
            routine_motion_clip(
                &routines,
                &actor.rejected_routines,
                DatId::from_str(routine),
            )
            .map(|d| d.as_str())
        };

        assert_eq!(clip("ati0").as_deref(), Some("at0?"), "swing routine");
        assert_eq!(clip("in 0").as_deref(), Some("ind?"), "draw routine");
        assert_eq!(clip("out0").as_deref(), Some("otd?"), "sheathe routine");
        assert_eq!(
            clip("cawh").as_deref(),
            Some("mw0?"),
            "white-magic cast routine"
        );

        let swing =
            routine_motion_clip(&routines, &actor.rejected_routines, DatId::from_str("ati0"))
                .unwrap();
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

    // The runtime releases a finished routine queue per ACTOR (scheduler_runtime's
    // stop path calls clear_cutscene_action on the render child), so held-motion
    // ownership does not need per-motion tracking: this pins that clearing drops
    // the held cast and the pose pass falls back to idle.
    #[test]
    fn clearing_the_held_scheduler_motion_falls_back_to_idle() {
        let Some(loaded) = load_hume_m() else { return };
        const CAST: [u8; 4] = *b"mw2?";
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        actor.begin_completion_motion(
            DatId::from_name(&CAST),
            CompletionMotion {
                local_clips: &[],
                duration_frames: 600.0,
                max_loops: 2,
                transition_in: HalfFrames::ZERO,
                transition_out: HalfFrames::ZERO,
            },
        );
        advance_actor_pose_standalone_locked(&mut actor, 10.0, true);
        assert!(actor.has_action());
        actor.clear_cutscene_action();
        assert!(!actor.has_action());
        advance_actor_pose_standalone_locked(&mut actor, 1.0, false);
        assert!(
            actor
                .coordinator
                .animations
                .iter()
                .flatten()
                .all(|slot| slot
                    .current_animation
                    .as_ref()
                    .is_none_or(|c| !c.animation.id.parameterized_match(&DatId::from_name(&CAST)))),
            "cleared cast must not survive a pose pass"
        );
    }

    // The runtime's stop path (scheduler_runtime) clears the held motion on the render
    // actor when its routine queue dies; either release route must hand the skeleton
    // back to the idle pose set rather than leaving the cast keyed.
    #[test]
    fn released_held_cast_leaves_the_pose_coordinator() {
        let Some(loaded) = load_hume_m() else { return };
        const CAST: [u8; 4] = *b"mw2?";
        for fresh_actor in [false, true] {
            let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
            if !fresh_actor {
                actor.begin_completion_motion(
                    DatId::from_name(&CAST),
                    CompletionMotion {
                        local_clips: &[],
                        duration_frames: 600.0,
                        max_loops: 2,
                        transition_in: HalfFrames::ZERO,
                        transition_out: HalfFrames::ZERO,
                    },
                );
            }
            advance_actor_pose_standalone_locked(&mut actor, 30.0, true);
            if fresh_actor {
                let still_held = actor.coordinator.animations.iter().flatten().any(|slot| {
                    slot.current_animation.as_ref().is_some_and(|c| {
                        c.animation.id.parameterized_match(&DatId::from_name(&CAST))
                    })
                });
                assert!(!still_held, "idle actor never held the cast");
                continue;
            }
            actor.clear_cutscene_action();
            advance_actor_pose_standalone_locked(&mut actor, 1.0, false);
            assert!(
                actor
                    .coordinator
                    .animations
                    .iter()
                    .flatten()
                    .all(|slot| slot.current_animation.as_ref().is_none_or(|c| !c
                        .animation
                        .id
                        .parameterized_match(&DatId::from_name(&CAST)))),
                "released cast still owns the rendered skeleton"
            );
        }
    }

    #[test]
    fn locomotion_clip_loops_continuously_across_updates() {
        let Some(loaded) = load_hume_m() else { return };
        let mut actor = make_render_actor(&loaded, 0, Vec::new(), 1, 0.0, 1.0);
        let inputs = inputs_for_pose(PoseState::Walk, false);

        const UPDATES: usize = 10;
        const FRAME_STEP: f32 = 24.0;

        // First update registers the walk clip set (ffxi-actor/src/animation.rs
        // SkeletonAnimationCoordinator). Capture current_clip and every registered non-idle
        // clip's cursor; an unchanged gait does not re-select or re-register them.
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

            // current_clip is written only inside the registration gate (ffxi-actor/src/animation.rs);
            // if it does not change across updates, register_animation was not called again ->
            // registered exactly once.
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
                // The cursor advances forward by exactly one frame step (mod length); a reset to 0
                // on an unchanged gait would break this. Both representatives of the residue are
                // accepted: SkeletonAnimationContext::apply_loop_bounds (ffxi-actor/src/animation.rs)
                // wraps with `> length`, so a cursor landing exactly on length holds there for one
                // step instead of reading 0 (the two sample identically at loop closure).
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

        let buffer = synth_buffer(&SAMPLES);
        let parts = PreparedParts {
            texture_names: Vec::new(),
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
            &[],
            &mut materials,
            &mut cache,
            &mut registry,
            &parts,
            root,
            skin_slot,
            None,
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root());
        let Some(base) = skeleton_file_id_for_race(dll.as_deref(), 1) else {
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = main_dll_for_root(root.root());
        let differs = (1u8..=8).any(|race| {
            let Some(base) = skeleton_file_id_for_race(dll.as_deref(), race) else {
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

    /// The dedupe key is (world_id, clip, reason): one CLIP_WARN line per entity per requested
    /// clip per diagnostic. Unique ids keep this test independent of any other test in the
    /// process sharing the global seen-set.
    #[test]
    fn clip_warn_prints_once_per_pair() {
        let id_a = 0xC0DE_0001;
        let id_b = 0xC0DE_0002;
        let wlk = DatId::from_str("wlk?");

        assert!(clip_warn_once(
            id_a,
            "dedupe-a",
            "ROM/4/999.DAT",
            &wlk,
            "not_found"
        ));
        assert!(
            !clip_warn_once(id_a, "dedupe-a", "ROM/4/999.DAT", &wlk, "not_found"),
            "second sighting of the same pair stays quiet"
        );
        assert!(
            clip_warn_once(id_a, "dedupe-a", "ROM/4/999.DAT", &wlk, "seq_load_error"),
            "a different reason on the same pair gets its own line"
        );
        assert!(
            clip_warn_once(id_b, "dedupe-b", "ROM/4/999.DAT", &wlk, "not_found"),
            "a different entity id gets its own line"
        );
    }
}

#[cfg(test)]
mod skin_slab_tests {
    use super::*;

    fn stub_skeleton(joints: usize) -> Skeleton {
        Skeleton {
            id: DatId::from_str("0000"),
            joints: (0..joints)
                .map(|i| ffxi_dat::skel::Joint {
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    translation: [i as f32, 0.0, 0.0],
                    parent: i.checked_sub(1),
                })
                .collect(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        }
    }

    /// The serial pose copy is the only thing that gets a pose onto the GPU; a wrong setter
    /// here freezes every character on its bind pose.
    #[test]
    fn live_tick_writes_pose_through_the_dirty_setter() {
        let joints = 5;
        let pose: Vec<Mat4> = (0..joints)
            .map(|i| Mat4::from_translation(Vec3::splat(i as f32 + 1.0)))
            .collect();

        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<FfxiSkinRegistry>()
            .add_systems(Update, tick_ffxi_render_actors);

        let slot = app
            .world_mut()
            .resource_mut::<FfxiSkinRegistry>()
            .alloc_skin();
        let mut actor = render_actor_for_test(stub_skeleton(joints), pose);
        actor.skin_slot = slot;
        app.world_mut().spawn(actor);

        app.update();

        let world_pose = app
            .world_mut()
            .query::<&FfxiRenderActor>()
            .single(app.world())
            .expect("one actor")
            .world_pose
            .clone();
        assert_eq!(world_pose.len(), joints);
        let reg = app.world().resource::<FfxiSkinRegistry>();
        assert_eq!(&reg.skin(slot).joints.matrices[..joints], &world_pose[..]);
    }
}

#[cfg(test)]
mod actor_dat_root_tests {
    use super::*;

    #[test]
    fn some_wired_root_is_reused_verbatim() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            eprintln!("skipping: no retail DAT root");
            return;
        };
        let root = Arc::new(root);
        let resolved =
            resolve_actor_root(Some(root.clone())).expect("a wired root always resolves");
        assert!(
            Arc::ptr_eq(&root, &resolved),
            "a wired root must be reused, not reopened into a new DatRoot"
        );
    }

    /// Bounded so a stuck task fails the test instead of hanging it; the load is one
    /// race-config DAT read plus a handful of marker scans, so this is orders of magnitude of
    /// slack over a real load.
    const KICK_LOAD_TASK_POLLS: usize = 600;
    const KICK_LOAD_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

    #[test]
    fn kick_load_actor_tasks_reuses_the_wired_root() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            eprintln!("skipping: no retail DAT root");
            return;
        };
        bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
        let root = Arc::new(root);
        let entity_id = 1u32;

        let mut app = App::new();
        app.add_message::<LoadActorRequest>()
            .init_resource::<crate::scene::TrackedEntities>()
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .init_resource::<ActorLoadInFlight>()
            .insert_resource(ActorDatRoot(Some(root.clone())))
            .add_systems(Update, kick_load_actor_tasks);

        let wire_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<crate::scene::TrackedEntities>()
            .by_id
            .insert(entity_id, wire_entity);
        app.world_mut().write_message(LoadActorRequest {
            entity_id,
            subject: ActorSubject::Pc {
                race: 1,
                mounted: false,
                equipment: Vec::new(),
                body: None,
                main_weapon: None,
                sub_weapon: None,
            },
        });
        app.update();

        for _ in 0..KICK_LOAD_TASK_POLLS {
            let done = {
                let mut in_flight = app.world_mut().resource_mut::<ActorLoadInFlight>();
                let Some(task) = in_flight.tasks.get_mut(&entity_id) else {
                    panic!("kick_load_actor_tasks did not register an in-flight task");
                };
                future::block_on(future::poll_once(task))
            };
            if let Some(result) = done {
                result.expect("load through the wired root must succeed");
                return;
            }
            std::thread::sleep(KICK_LOAD_POLL_INTERVAL);
        }
        panic!("kick_load_actor_tasks task never completed");
    }
}
