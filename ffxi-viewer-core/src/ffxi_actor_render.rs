//! Phase 3 — XIM-faithful character render path sourced from the NEW
//! `ffxi-dat` parsers (`skel`, `skel_mesh`, `skel_anim`, `resource_dir`) and
//! the `ffxi-actor` runtime, fed to the EXISTING custom skinned material.
//!
//! This module is self-contained and additive: it loads a subject (an NPC by
//! primary DAT file_id, or a PC by race + equipment file_ids), builds one
//! Bevy `Mesh` per `MeshBuffer`, resolves textures by name, drops occluded
//! buffers exactly like XIM `ActorModel.isOccluded`, and drives the pose each
//! frame through `ffxi_actor::skeleton_instance::pose_world`.
//!
//! Unlike the legacy `dat_vos2` path it does NOT bake bone matrices on the CPU
//! up front, does NOT min_y-pivot the feet, and does NOT roll bone 0: the new
//! `pose_world` already encodes facing/scale (via `RootTransform`) and the new
//! `skel` parser keeps the root rotation, so the only placement transform is a
//! single FFXI->Bevy basis change on the actor-root entity (the shader's
//! `world_from_local`).

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

use ffxi_actor::actor_state::{self, ActorAnimInputs};
use ffxi_actor::animation::{LoopParams, SkeletonAnimationCoordinator};
use ffxi_actor::skeleton_instance::{pose_world, RootTransform};

use ffxi_dat::d3m::D3m;
use ffxi_dat::datid::DatId;
use ffxi_dat::resource_dir::ResourceDir;
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

// ---------------------------------------------------------------------------
// Live-client wiring: messages + per-entity render-root link
// ---------------------------------------------------------------------------

/// What an entity should be rendered as on the faithful path. Resolved by
/// `look_resolver::dispatch_look_driven_models` from the wire `EntityLook`:
/// `Equipped` → `Pc`, `Standard` → `Npc`.
#[derive(Debug, Clone)]
pub enum ActorSubject {
    /// A player character: the race skeleton DAT plus the resolved equipment
    /// (face + per-slot) file_ids, in the order `load_pc` expects.
    Pc { race: u8, equipment: Vec<u32> },
    /// A fixed NPC: the single actor DAT file_id (already through `npc_dat_id`).
    Npc { file_id: u32 },
}

/// Request to (re)build the faithful render-root for one wire entity. Fired
/// by the look dispatcher (one per entity, replacing the per-slot/per-chunk
/// `LoadVos2Request` fan-out) and consumed by [`process_load_actor_requests`].
/// Same derive/registration style as `dat_vos2::LoadVos2Request`.
#[derive(Message, Debug, Clone)]
pub struct LoadActorRequest {
    pub entity_id: u32,
    pub subject: ActorSubject,
}

/// Marks a wire `WorldEntity` whose faithful render-root has been spawned,
/// storing that root entity so a later look change can despawn it (and its
/// descendants) before spawning a replacement.
#[derive(Component, Debug, Clone, Copy)]
pub struct FfxiRenderRoot(pub Entity);

/// FFXI authoring rate: animation key frames advance ~30 per second. The
/// runtime's `get_joint_transform` already scales by `key_frame_duration`, so
/// the tick feeds it `elapsed_seconds * FRAME_RATE` as an elapsed-frames count.
pub const FRAME_RATE: f32 = 30.0;

/// FFXI skeleton-space -> Bevy basis. The new `pose_world` leaves the rig in
/// FFXI engine space (Y-down-ish: a Y-up Bevy camera looks at the rig
/// upside-down without this). `Q_x(pi)` flips it upright; empirically this
/// stands Galka/Mithra/NPCs vertical and facing the camera at facing_dir = 0.
/// No `Q_y` roll-cancel is needed here because the new `skel` parser preserves
/// the root joint rotation that `pose_world`'s root branch consumes.
fn ffxi_to_bevy_basis() -> Quat {
    Quat::from_rotation_x(std::f32::consts::PI)
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// One named, decoded texture from an `Img` chunk colocated with the meshes.
struct NamedTexture {
    name: String,
    texture: DecodedTexture,
}

/// Everything needed to render + animate one actor, parsed from the relevant
/// DAT files via the NEW parsers.
pub struct LoadedActor {
    pub skeleton: Arc<Skeleton>,
    /// Every `SkelMesh` across the subject's DATs (NPC: one DAT; PC: skeleton
    /// DAT + one per equipment file_id). Bone indices reference the shared
    /// race/actor skeleton.
    pub skel_meshes: Vec<SkelMesh>,
    /// `D3M` effect-billboard chunks (kind 0x1F) colocated with the meshes.
    /// Some NPCs (e.g. the fire elemental 1308) carry essentially no skinned
    /// body — their visible form lives entirely in these effect meshes, which
    /// XIM spawns as particles. We render them statically (rigidly bound to the
    /// emitter root) so the subject is at least visible, sourcing the same
    /// `ele_*` textures the particle path would.
    effect_meshes: Vec<D3m>,
    /// Decoded textures keyed by their `Img` name across every loaded DAT.
    textures: Vec<NamedTexture>,
    /// `find_animations_matching` source: every DAT we loaded, kept so the
    /// per-frame tick can resolve `idl?`/`run?`/... against all of them.
    anim_dirs: Vec<ResourceDir>,
}

/// Read a DAT file's bytes by file_id, or `None` when it can't be resolved.
fn read_dat(root: &DatRoot, file_id: u32) -> Option<Vec<u8>> {
    let loc = root.resolve(file_id).ok()?;
    fs::read(loc.path_under(root.root())).ok()
}

/// XIM keys a texture by its full 16-char `nextString(0x10)` name, which sits
/// at `body[1..0x11]` (right after the 1-byte type/`flg`). The mesh side stores
/// the *same* 16-char field in `MeshBuffer.texture_name`, so keying both with
/// this raw 16-char name (and a localName fallback, [`TextureKey`]) matches
/// XIM's `getTextureResourceByNameAs` two-tier lookup. (Note: this is the FULL
/// `nameSpace+localName` field; `texture::extract_texture_name` returns only
/// the trimmed localName and is intentionally NOT used here.)
fn full_texture_name(body: &[u8]) -> String {
    body.get(1..0x11)
        .map(|raw| raw.iter().map(|&b| b as char).collect())
        .unwrap_or_default()
}

/// Recursively collect every `Img`-chunk texture in a tree, keyed by name.
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

/// Recursively collect the renderable `D3M` (0x1F) effect meshes in a tree.
///
/// A subject's effect DATs hold two kinds of D3M: the structured *body* mesh
/// (the elemental's flame, dozens-to-hundreds of triangles in a sub-unit
/// volume) and 2-triangle full `[-1,1]` *glow/smoke billboards* (`mowa`,
/// `pou`) that XIM only ever draws as additive/blended particles. Rendered
/// opaque the billboards become a giant white card that hides the body, so we
/// keep only the structured bodies (`num_triangles > 2`).
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

/// Find the first skeleton (0x29) in a DAT buffer via the resource dir.
fn first_skeleton(bytes: &[u8]) -> Option<Skeleton> {
    ResourceDir::from_bytes(bytes.to_vec())
        .collect_skeletons()
        .into_iter()
        .next()
}

/// Load an NPC: skeleton + meshes + textures + animations all live in one DAT.
pub fn load_npc(file_id: u32) -> Result<LoadedActor, String> {
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

    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,
        effect_meshes,
        textures,
        anim_dirs: vec![ResourceDir::from_bytes(bytes)],
    })
}

/// Resolve a race's *default naked* equipment file_ids: face + the head /
/// body / hands / legs / feet base meshes (slot nibble 1..5, item id 0 = the
/// "naked" model for that slot). The race skeleton DAT carries the skeleton +
/// animations but no body geometry, so a PC ALWAYS needs these to have a body.
fn default_pc_equipment(race: u8) -> Vec<u32> {
    use crate::look_resolver::{resolve_equipment_slot, resolve_face};
    let mut out = Vec::new();
    if let Some(f) = resolve_face(1, race) {
        out.push(f);
    }
    // head=0x1000, body=0x2000, hands=0x3000, legs=0x4000, feet=0x5000 (id=0).
    for slot in 1u16..=5 {
        if let Some(f) = resolve_equipment_slot(slot << 12, race) {
            out.push(f);
        }
    }
    out
}

/// Load a PC: the race skeleton comes from `PCSkeletonIDs`, the equipment
/// meshes/textures from separate DATs (all skinned to the shared race
/// skeleton). `equipment` is the list of resolved equipment-slot/face file_ids;
/// when empty, the race's default naked-body slots are resolved automatically.
pub fn load_pc(race: u8, equipment: &[u32]) -> Result<LoadedActor, String> {
    let root = DatRoot::from_env_or_default().map_err(|e| format!("DatRoot: {e}"))?;
    let skel_file_id =
        skeleton_file_id_for_race(race).ok_or_else(|| format!("unsupported race {race}"))?;

    // The skeleton DAT also carries the race's animations (idl/run/...).
    let skel_bytes =
        read_dat(&root, skel_file_id).ok_or_else(|| format!("read skel dat {skel_file_id}"))?;
    let skeleton = first_skeleton(&skel_bytes)
        .ok_or_else(|| format!("no skeleton in race dat {skel_file_id}"))?;

    let mut skel_meshes = Vec::new();
    let mut textures = Vec::new();
    let mut anim_dirs = vec![ResourceDir::from_bytes(skel_bytes.clone())];

    // The skeleton DAT can itself carry base body meshes (the "naked" race
    // model); include them so an equipment-less PC still has a body.
    {
        let dir = ResourceDir::from_bytes(skel_bytes.clone());
        skel_meshes.extend(dir.collect_skel_meshes());
        collect_textures(&walk_tree(&skel_bytes), &mut textures);
    }

    let resolved_default;
    let equipment = if equipment.is_empty() {
        resolved_default = default_pc_equipment(race);
        resolved_default.as_slice()
    } else {
        equipment
    };

    // Per-equipment-file mesh-count trace. A live PC that renders "head only"
    // (reported for Taru) drops here in one of two ways: an equipment file that
    // can't be read (`read`), or one that parses but yields no skel meshes
    // (`0 meshes`). Logging the count per file_id pinpoints which slot/item
    // is the culprit so a live look at the actor diagnoses it precisely.
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

    // Full-rig (`*1` LOD) locomotion + battle clips live in the race's MOTION
    // DAT (skel + 2600), NOT the skeleton DAT — which carries only the low-LOD
    // `*0` clips (~12 joints, legs + spine, no arms). Without these the upper
    // body never animates while running (`run?` would resolve to the legs-only
    // `run0`); battle idle (`btl?`) and the full-rig death (`cor1`) are also
    // motion-DAT-only. The motion DAT holds no skeleton/meshes, just clips.
    if let Some(motion_id) = combat_stance::motion_dat_for_skel(skel_file_id) {
        if let Some(bytes) = read_dat(&root, motion_id) {
            anim_dirs.push(ResourceDir::from_bytes(bytes));
        }
    }

    if skel_meshes.is_empty() {
        return Err(format!(
            "no skeleton meshes for race {race} equipment {equipment:?}"
        ));
    }

    Ok(LoadedActor {
        skeleton: Arc::new(skeleton),
        skel_meshes,
        // PCs have no effect meshes (their body is all skinned geometry).
        effect_meshes: Vec::new(),
        textures,
        anim_dirs,
    })
}

// ---------------------------------------------------------------------------
// Occlusion (XIM ActorModel.isOccluded)
// ---------------------------------------------------------------------------

/// XIM `ActorModel.isOccluded`: a buffer with a `display_type_flag` in
/// {1,2,3 hair / 4 face / 5 wrist / 6 pants / 7 shins} is dropped when the
/// actor's set of `occlude_type`s contains the corresponding value(s).
fn is_occluded(buffer: &MeshBuffer, occlusion: &std::collections::HashSet<u8>) -> bool {
    let has = |v: u8| occlusion.contains(&v);
    match buffer.render_properties.display_type_flag {
        0 => false,
        // hair 1
        1 => has(0x02) || has(0x03) || has(0x04) || has(0x05) || has(0x06),
        // hair 2 / hair 3
        2 | 3 => has(0x04) || has(0x05) || has(0x06),
        // face
        4 => has(0x05),
        // wrist
        5 => has(0x12),
        // pants
        6 => has(0x32),
        // shins
        7 => has(0x22),
        // Unknown display types render (XIM throws; we keep them visible).
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Mesh build
// ---------------------------------------------------------------------------

/// One built mesh group ready to spawn: a Bevy mesh handle + its material.
struct BuiltGroup {
    mesh: Mesh,
    texture_name: String,
}

/// Clamp a wire joint index into the skeleton's bone range. Out-of-range
/// indices (rare race/equipment mismatches) bind to bone 0 (the hip) so a
/// stray vertex doesn't read a garbage matrix.
fn clamp_joint(idx: u16, joint_count: usize) -> u32 {
    let i = idx as usize;
    if i < joint_count {
        i as u32
    } else {
        0
    }
}

/// Build one Bevy `Mesh` for a single `MeshBuffer`. Vertices are already in
/// draw order, so indices are sequential `0..n` for both strip and mesh.
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
        // The shader takes a single bone-0 weight `w`; bone-1 weight is
        // `1 - w`. The parser stores both; use joint0_weight as `w`.
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

/// Build one Bevy `Mesh` from a `D3M` effect chunk. D3M is a flat triangle
/// list of billboard verts authored in the emitter's local space; we render it
/// statically, rigidly bound to the emitter root (bone 0 — every emitter joint
/// of these rigs sits at the origin), so `w = 1`, `position1/normal1 = 0`. The
/// /128 HDR color is clamped to LDR for the opaque pass.
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

/// XIM `TextureName` (DatResource.kt): split a fully-qualified 16-char texture
/// name into `nameSpace = name[0..8]` and `localName = name[8..16]`, each
/// trimmed of trailing NUL/space. Shorter strings (no namespace) put their
/// whole content in `local_name`, leaving `name_space` empty — so the
/// localName fallback still resolves them.
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

    /// Combined `nameSpace/localName` key for the full-match HashMap tier.
    fn full_key(&self) -> String {
        format!("{}/{}", self.name_space, self.local_name)
    }
}

/// Whether a mesh `texture_name` field is blank — XIM gives such meshes a null
/// `TextureLink` (untextured vertex-colored C/CS ops) and renders the vertex
/// color directly rather than sampling an arbitrary texture.
fn is_blank_texture(name: &str) -> bool {
    name.trim_matches(['\0', ' ']).is_empty()
}

// ---------------------------------------------------------------------------
// ECS component + spawn
// ---------------------------------------------------------------------------

/// Per-actor render + animation state. Holds the skeleton + animations and the
/// per-group material handles whose bone uniform the tick rewrites each frame.
#[derive(Component)]
pub struct FfxiRenderActor {
    pub skeleton: Arc<Skeleton>,
    /// All animations from the actor's DATs, kept resolved so the tick can
    /// `parameterized_match` a selected `idl?`/`run?`/... id every frame.
    animations: Vec<SkeletonAnimation>,
    coordinator: SkeletonAnimationCoordinator,
    materials: Vec<Handle<FfxiSkinnedMaterial>>,
    /// Current animation-selection inputs (set by the example harness).
    pub inputs: ActorAnimInputs,
    /// Live link to the wire entity id, used by [`tick_live_ffxi_actors`] to
    /// look up motion / engagement / rest each frame. `0` for the example
    /// harness (which drives `inputs` directly and never queries live state).
    pub world_id: u32,
    /// Actor facing in radians (root yaw), applied via `RootTransform`.
    pub facing_dir: f32,
    /// Uniform scale applied to the whole skeleton via the root.
    pub scale: f32,
    /// The id currently registered into the coordinator, so we only re-register
    /// on a real change.
    current_clip: Option<DatId>,
    /// Diagnostics for the example overlay.
    pub last_clip: Option<DatId>,
    pub last_frame: f32,
}

impl LoadedActor {
    /// Flatten every loaded DAT's animations into one list.
    fn all_animations(&self) -> Vec<SkeletonAnimation> {
        let mut out = Vec::new();
        for dir in &self.anim_dirs {
            out.extend(dir.collect_animations());
        }
        out
    }

    /// Bind-pose world-space AABB of all *rendered* (non-occluded) vertices,
    /// in the SAME Bevy frame the spawned actor lives in (the FFXI->Bevy basis
    /// flip is applied). Returns `None` when nothing renders.
    ///
    /// This lets a harness frame any subject by its real geometry rather than a
    /// fixed humanoid height. Some subjects are NOT humanoid: the fire-elemental
    /// (NPC 1308) is a flat ~1-unit emitter disk of sub-millimeter particle-
    /// anchor triangles sitting at y≈0 — a fixed torso-height camera misses it
    /// entirely, so auto-framing is the only way to even see what little skinned
    /// geometry it has (its flame body is a particle effect we don't draw).
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
        // Effect (D3M) meshes are bound rigidly to the emitter root (bone 0).
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

/// Spawn a loaded actor under `parent`, at `world_pos`. Returns the spawned
/// actor-root entity (which carries the FFXI->Bevy basis + position). The
/// material handles are stored on the inserted [`FfxiRenderActor`] component so
/// the per-frame tick can rewrite their bone uniform.
pub fn spawn_loaded_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    world_pos: Vec3,
    facing_dir: f32,
    scale: f32,
) -> Entity {
    // Actor-root entity carries the single FFXI->Bevy basis + world position.
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

    let material_handles = build_actor_children(
        commands, meshes, materials, images, loaded, actor_root, facing_dir, scale,
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

/// Build + attach every mesh-group (and effect-mesh) child of `actor_root`
/// from `loaded`, returning the per-group material handles. The actor-root
/// itself (with its FFXI->Bevy basis transform + parenting) is set up by the
/// caller; this is the geometry/material body shared by [`spawn_loaded_actor`]
/// (free harness root) and [`spawn_live_actor`] (root parented to a wire
/// entity).
#[allow(clippy::too_many_arguments)]
fn build_actor_children(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    actor_root: Entity,
    facing_dir: f32,
    scale: f32,
) -> Vec<Handle<FfxiSkinnedMaterial>> {
    // Texture pool keyed XIM-style: a full `nameSpace/localName` map (tier 1)
    // and a `localName`-only map (tier 2 fallback), both filled from each
    // texture's own full 16-char name. A third `by_trimmed` map keys the WHOLE
    // name with only trailing NUL/space stripped (no split-at-8) — D3M effect
    // chunks store the short, un-split name (`ele_firehono`), so the split-based
    // tiers miss them (localName would be `hono`).
    let mut by_full: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(loaded.textures.len());
    let mut by_local: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(loaded.textures.len());
    let mut by_trimmed: std::collections::HashMap<String, Handle<Image>> =
        std::collections::HashMap::with_capacity(loaded.textures.len());
    for nt in &loaded.textures {
        let handle = images.add(decoded_texture_to_image(&nt.texture));
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

    // Actor-wide occlusion set across every loaded SkelMesh.
    let occlusion: std::collections::HashSet<u8> =
        loaded.skel_meshes.iter().map(|m| m.occlude_type).collect();

    let joint_count = loaded.skeleton.joints.len();

    // Seed each material's bone uniform with the bind pose; the tick
    // overwrites it from the animated pose each frame.
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

    let mut material_handles = Vec::new();

    for skel_mesh in &loaded.skel_meshes {
        for buffer in &skel_mesh.meshes {
            if buffer.vertices.is_empty() {
                continue;
            }
            if is_occluded(buffer, &occlusion) {
                continue;
            }
            let built = BuiltGroup {
                mesh: build_mesh(buffer, joint_count),
                texture_name: buffer.texture_name.clone(),
            };
            // Untextured (C/CS) buffers have a blank texture_name -> null
            // TextureLink in XIM: bind no texture and flag the material so the
            // shader renders the vertex color instead of an arbitrary texture.
            let untextured = is_blank_texture(&built.texture_name);
            let tex_handle = if untextured {
                None
            } else {
                resolve_texture(&built.texture_name)
            };
            let has_texture = if tex_handle.is_some() { 1.0 } else { 0.0 };

            let mat = materials.add(FfxiSkinnedMaterial {
                lighting: FfxiLightingUniform::default(),
                base_color_texture: tex_handle,
                joints: bind_joints.clone(),
                material_flags: FfxiMaterialFlags {
                    flags: Vec4::new(has_texture, 0.0, 0.0, 0.0),
                },
            });
            material_handles.push(mat.clone());

            commands.spawn((
                Mesh3d(meshes.add(built.mesh)),
                MeshMaterial3d(mat),
                Transform::default(),
                ChildOf(actor_root),
            ));
        }
    }

    // Effect (D3M) meshes: the elemental's flame body and similar subjects whose
    // visible form is particle geometry, not skinned body. Rendered statically,
    // bound to the emitter root (bone 0), with their `ele_*` texture. The bone-0
    // bind matrix already carries the actor facing/scale.
    for d3m in &loaded.effect_meshes {
        if d3m.vertices.is_empty() {
            continue;
        }
        let tex_handle = resolve_texture(&d3m.texture_name_str());
        let has_texture = if tex_handle.is_some() { 1.0 } else { 0.0 };
        let mat = materials.add(FfxiSkinnedMaterial {
            lighting: FfxiLightingUniform::default(),
            base_color_texture: tex_handle,
            joints: bind_joints.clone(),
            material_flags: FfxiMaterialFlags {
                flags: Vec4::new(has_texture, 0.0, 0.0, 0.0),
            },
        });
        material_handles.push(mat.clone());
        commands.spawn((
            Mesh3d(meshes.add(build_d3m_mesh(d3m))),
            MeshMaterial3d(mat),
            Transform::default(),
            ChildOf(actor_root),
        ));
    }

    material_handles
}

/// Assemble the [`FfxiRenderActor`] component from a loaded subject + the
/// material handles built for it. Shared by the harness spawn (world_id 0)
/// and the live spawn (a real wire id).
fn make_render_actor(
    loaded: &LoadedActor,
    materials: Vec<Handle<FfxiSkinnedMaterial>>,
    world_id: u32,
    facing_dir: f32,
    scale: f32,
) -> FfxiRenderActor {
    FfxiRenderActor {
        skeleton: loaded.skeleton.clone(),
        animations: loaded.all_animations(),
        coordinator: SkeletonAnimationCoordinator::new(),
        materials,
        inputs: ActorAnimInputs::default(),
        world_id,
        facing_dir,
        scale,
        current_clip: None,
        last_clip: None,
        last_frame: 0.0,
    }
}

/// Spawn a loaded actor as a CHILD of `wire_entity` (the tracked `WorldEntity`),
/// so the rig inherits the wire entity's world position AND heading rotation —
/// exactly like the legacy VOS2 path parents its pivot to the wire entity. The
/// actor-root's local transform is `translation = ZERO` (position comes from the
/// parent) and `rotation = ffxi_to_bevy_basis()` (the single FFXI->Bevy basis;
/// the parent's `Q_y(-heading)` then composes on top, turning the rig to face
/// the wire heading). `facing_dir` is held at `0.0` here: heading is carried by
/// the inherited parent rotation, NOT by `RootTransform`.
///
/// Returns the spawned actor-root entity. The caller records it in a
/// [`FfxiRenderRoot`] marker on the wire entity so a later look change can
/// despawn it.
///
/// TUNABLES (coordinate frame): see [`ffxi_to_bevy_basis`]. Because the rig
/// inherits the wire heading via parenting + the `Q_x(π)` basis while keeping
/// `facing_dir = 0`, the new `pose_world` retains the root-joint roll that the
/// legacy pivot canceled with a `Q_y(π/2)` — so the character may render at a
/// fixed yaw offset (e.g. 90°/180°) from correct. Adjust by composing a yaw into
/// [`ffxi_to_bevy_basis`] (see its doc). `scale` is passed by the caller
/// (currently `1.0`); feet-on-ground relies on the wire position being the
/// ground point (this path applies no `min_y` pivot).
#[allow(clippy::too_many_arguments)]
pub fn spawn_live_actor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
    images: &mut Assets<Image>,
    loaded: &LoadedActor,
    wire_entity: Entity,
    world_id: u32,
    scale: f32,
) -> Entity {
    // facing_dir stays 0: the parent (wire entity) carries the heading.
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
        commands, meshes, materials, images, loaded, actor_root, facing_dir, scale,
    );

    commands.entity(actor_root).insert(make_render_actor(
        loaded,
        material_handles,
        world_id,
        facing_dir,
        scale,
    ));

    actor_root
}

/// `dat_vos2::decoded_texture_to_image` is private; re-derive the same upload
/// here so this module stays self-contained.
fn decoded_texture_to_image(t: &DecodedTexture) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
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

// ---------------------------------------------------------------------------
// Animation tick
// ---------------------------------------------------------------------------

/// Per-frame tick: select a clip from the actor inputs, register it into the
/// coordinator, advance, evaluate the world pose, and stamp it into every
/// group material's bone uniform.
pub fn tick_ffxi_render_actors(
    time: Res<Time>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut q_actors: Query<&mut FfxiRenderActor>,
) {
    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    for mut actor in &mut q_actors {
        advance_actor_pose(&mut actor, elapsed_frames, &mut materials);
    }
}

/// Shared per-actor pose pipeline used by both [`tick_ffxi_render_actors`]
/// (harness) and [`tick_live_ffxi_actors`] (live): clip selection ->
/// coordinator register/update -> `pose_world` -> stamp every group material's
/// bone uniform. The caller sets `actor.inputs` / `actor.facing_dir` first;
/// this function only reads them.
fn advance_actor_pose(
    actor: &mut FfxiRenderActor,
    elapsed_frames: f32,
    materials: &mut Assets<FfxiSkinnedMaterial>,
) {
    // Rest postures (sit/kneel/heal) play a dedicated model routine
    // (chi0/res0), which XIM selects outside `selected_animation`. When a
    // rest is active, prefer its id; otherwise fall through to the
    // idle-vs-movement selection. `idle` here just controls low-priority.
    let (selected_id, is_idle) = match actor_state::rest_animation_id(actor.inputs.rest) {
        Some(rest_id) => (rest_id, true),
        None => {
            let s = actor_state::selected_animation(&actor.inputs);
            (s.id, s.idle)
        }
    };

    // Resolve the selected parameterized id to ALL matching clips and register
    // each into its final-digit slot (XIM `fetchAnimations` + per-clip
    // `registerAnimation`). FFXI splits one pose across DISJOINT body-region
    // slots keyed by the clip's final digit: e.g. `run0` (slot 0) animates only
    // the legs/feet (joints 25..37) and `run1` (slot 1) only the spine/arms/
    // head (3..24, 49..88) — non-overlapping sets the coordinator composites
    // per-joint into a full-body pose. Registering just one slot animates only
    // that half ("running only torso-down"). And a slot left occupied by the
    // PREVIOUS pose keeps playing it (idle is `idl0`, slot 0 only, so a stale
    // slot 1 keeps the arms running after the legs stop) — so on a pose change
    // clear every slot first, then register the whole set. Fall back to `idl?`
    // when nothing matches so the actor never freezes blank.
    let matches: Vec<SkeletonAnimation> = {
        let m: Vec<SkeletonAnimation> = actor
            .animations
            .iter()
            .filter(|a| a.id.parameterized_match(&selected_id))
            .cloned()
            .collect();
        if m.is_empty() {
            let idle = DatId::from_str("idl?");
            actor
                .animations
                .iter()
                .filter(|a| a.id.parameterized_match(&idle))
                .cloned()
                .collect()
        } else {
            m
        }
    };

    // Re-register only when the SELECTED id changes, so steady-state looping
    // keeps each slot's frame cursor (and the legs/arms stay phase-locked).
    if !matches.is_empty() && actor.current_clip != Some(selected_id) {
        actor.current_clip = Some(selected_id);
        actor.coordinator.clear();
        let loop_params = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: is_idle,
        };
        for clip in &matches {
            actor
                .coordinator
                .register_animation(clip.clone(), loop_params, None, |_| true);
        }
    }
    // Overlay diagnostic: the fullest registered clip's id.
    actor.last_clip = matches.iter().max_by_key(|a| a.key_frame_sets.len()).map(|a| a.id);

    actor.coordinator.update(elapsed_frames);
    // Surface the high slot's frame cursor for the overlay.
    actor.last_frame = actor
        .coordinator
        .animations
        .iter()
        .flatten()
        .filter_map(|a| a.current_animation.as_ref().map(|c| c.current_frame))
        .next_back()
        .unwrap_or(0.0);

    // Build the get_anim closure from the coordinator and evaluate.
    let pose = {
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

    for handle in &actor.materials {
        if let Some(m) = materials.get_mut(handle) {
            m.joints.set_from(&pose);
        }
    }
}

// ---------------------------------------------------------------------------
// Live-client systems
// ---------------------------------------------------------------------------

/// Drain [`LoadActorRequest`]s: (re)build the faithful render-root for each
/// requested wire entity. Resolves the wire `Entity` from `TrackedEntities`,
/// despawns any prior [`FfxiRenderRoot`] (and its descendants), builds the
/// `LoadedActor` for the subject, and spawns a new root parented to the wire
/// entity. Load failures are logged and skipped (never panic).
pub fn process_load_actor_requests(
    mut events: MessageReader<LoadActorRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_existing: Query<&FfxiRenderRoot>,
) {
    for req in events.read() {
        let Some(&wire_entity) = tracked.by_id.get(&req.entity_id) else {
            // Entity not tracked yet (look arrived before the spawn). The look
            // dispatcher re-fires on `Changed<LookComp>`, so this self-heals.
            continue;
        };

        // Despawn any previously-spawned render-root for this entity. In Bevy
        // 0.18 `despawn` is recursive, so the root's mesh/material children go
        // with it.
        if let Ok(FfxiRenderRoot(old_root)) = q_existing.get(wire_entity) {
            commands.entity(*old_root).despawn();
        }

        let loaded = match &req.subject {
            ActorSubject::Npc { file_id } => load_npc(*file_id),
            ActorSubject::Pc { race, equipment } => load_pc(*race, equipment),
        };
        let loaded = match loaded {
            Ok(l) => l,
            Err(e) => {
                warn!("ffxi actor load failed (entity {}): {e}", req.entity_id);
                continue;
            }
        };

        // Scale is 1.0 for now (TUNABLE: see `spawn_live_actor`).
        let root = spawn_live_actor(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &loaded,
            wire_entity,
            req.entity_id,
            1.0,
        );
        // Hide the placeholder debug capsule (`scene::sync_entities_system`
        // gives every wire entity a colored `Mesh3d` proxy): now that the
        // faithful model is parented under the wire entity, the proxy would
        // render as a solid capsule ON TOP of it. Drop the `Mesh3d` exactly
        // like the legacy path did on model load (`dat_vos2.rs:709`); the
        // `MeshMaterial3d` left behind is inert without geometry. Visibility
        // is untouched, so the child model still shows (and self-in-first-
        // person hiding via the wire entity's `Visibility` still works).
        // `try_insert`: the wire entity may despawn between drain and flush.
        commands
            .entity(wire_entity)
            .remove::<Mesh3d>()
            .try_insert(FfxiRenderRoot(root));
    }
}

/// Live counterpart to [`tick_ffxi_render_actors`]: build [`ActorAnimInputs`]
/// from live game state (motion / engagement / rest / dead) for each actor
/// with `world_id != 0`, then run the IDENTICAL pose pipeline. `facing_dir`
/// stays `0.0` — the parent (wire entity) carries the heading.
pub fn tick_live_ffxi_actors(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    motion: Res<combat_stance::EntityMotion>,
    rest: Res<combat_stance::RestStance>,
    walk_mode: Res<combat_stance::WalkMode>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut q_actors: Query<&mut FfxiRenderActor>,
) {
    use ffxi_actor::actor_state::{EngageAnimationState, RestKind};

    let elapsed_frames = time.delta_secs() * FRAME_RATE;
    let self_id = state.snapshot.self_char_id;

    // Once-per-frame indices so the per-actor lookups stay O(1) in crowded
    // zones (engagement + dead both scrape the snapshot entity list).
    let bt_target_by_id: std::collections::HashMap<u32, u32> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.bt_target_id))
        .collect();
    // Dead source: the wire `Entity.hp_pct`. `Some(0)` = at 0 HP (dead);
    // `None`/non-zero = alive. (`hp_pct` is the only death signal on the wire
    // today — there is no separate dead bool.)
    let dead_by_id: std::collections::HashMap<u32, bool> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.hp_pct == Some(0)))
        .collect();

    for mut actor in &mut q_actors {
        let world_id = actor.world_id;
        if world_id == 0 {
            // Harness-style actor under the live tick — leave to the harness
            // tick (not registered live) and skip.
            continue;
        }

        let is_self = Some(world_id) == self_id;
        let sample = motion.sample(world_id).unwrap_or_default();
        let engaged = bt_target_by_id
            .get(&world_id)
            .map(|&t| t != 0)
            .unwrap_or(false);
        let dead = dead_by_id.get(&world_id).copied().unwrap_or(false);
        // Rest stance is a self-only local affordance (`/sit` etc.); other
        // entities never carry it on the wire. Map the viewer-core
        // `combat_stance::RestKind` onto the `ffxi-actor` selection enum.
        let rest_kind = if is_self {
            match rest.kind {
                combat_stance::RestKind::None => RestKind::None,
                combat_stance::RestKind::Sit => RestKind::Sit,
                combat_stance::RestKind::Heal => RestKind::Heal,
            }
        } else {
            RestKind::None
        };

        // XIM lock/strafe gate (`actor_state::movement_direction` contract): a
        // free-moving actor turns to face where it's going, so its velocity is
        // purely forward and the clip is `run?`/`wlk?`. The sideways/backward
        // strafe clips (`mvl?`/`mvr?`/`mvb?`) only apply when the actor's
        // facing is pinned to a target while it moves — i.e. engaged. Feeding
        // the raw projection for a free runner lets a stale wire heading
        // project a forward run onto a strafe clip: legs splayed sideways with
        // the arms left in bind. Self's heading is client-driven and not echoed
        // back into the snapshot each frame, so its projection is never
        // trustworthy; only use the projection for engaged *remote* actors,
        // whose server-authored heading is authoritative.
        let (forward_vel, strafe_vel) = if engaged && !is_self {
            (sample.forward_component, sample.strafe_component)
        } else {
            (0.0, 0.0)
        };
        // /walk is a self-only local toggle; remote actors carry no wire
        // signal for it, so they always use the run gait.
        let walking = is_self && walk_mode.walking;

        actor.facing_dir = 0.0; // heading carried by the parent rotation.
        actor.inputs = ActorAnimInputs {
            moving: motion.is_moving(world_id),
            walking,
            forward_vel,
            strafe_vel,
            heading_rate: sample.heading_rate,
            engage_state: if engaged {
                EngageAnimationState::Engaged
            } else {
                EngageAnimationState::NotEngaged
            },
            dead,
            rest: rest_kind,
            ..Default::default()
        };

        advance_actor_pose(&mut actor, elapsed_frames, &mut materials);
    }
}

/// Per-frame: upload the live zone sun / moon / ambient into every faithful
/// render-actor material's light uniform, and stamp the realistic-lighting
/// toggle (`GraphicsSettings::realistic_character_lighting`) into the material
/// flag the shader branches on.
///
/// Replaces `dat_vos2::update_ffxi_lighting_system`, which queried the retired
/// `FfxiActor` component and so never reached the new live `FfxiRenderActor`
/// materials — leaving them on the flat neutral default uniform. The lux→unit
/// mapping matches that system so faithful shading is unchanged; only the
/// target component (and the realistic flag) differ.
pub fn update_ffxi_render_actor_lighting(
    settings: Res<crate::graphics_settings::GraphicsSettings>,
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
    q_actors: Query<&FfxiRenderActor>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    // Reference scales (matched to `dat_vos2::update_ffxi_lighting_system`):
    // GlobalAmbientLight defaults to ~500 lux; the sun curve peaks ~10k lux
    // at noon. Map both into the shader's ~0..1 contribution band.
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
        // Zone point lights aren't wired into the faithful path yet; a zeroed
        // `.w` (range) makes the shader's point loop skip every slot.
        point_pos: [Vec4::ZERO; 4],
        point_color: [Vec4::ZERO; 4],
    };

    let realistic = if settings.realistic_character_lighting {
        1.0
    } else {
        0.0
    };

    for actor in &q_actors {
        for h in &actor.materials {
            if let Some(m) = materials.get_mut(h) {
                m.lighting = lighting.clone();
                // Preserve `.x` (has_texture); only drive the realistic flag.
                m.material_flags.flags.y = realistic;
            }
        }
    }
}

/// Convenience for the examples: a registration of [`tick_ffxi_render_actors`]
/// is left to the caller's `add_systems` so this module imposes no plugin.
pub fn add_tick_system(app: &mut App) {
    app.add_systems(Update, tick_ffxi_render_actors);
}

// ---------------------------------------------------------------------------
// Pose state helpers for the example harness
// ---------------------------------------------------------------------------

/// Named pose state the examples expose (1=idle .. 0=dead, plus engaged).
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

    /// Parse a CLI/keyboard pose-name into a state.
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

/// Translate a [`PoseState`] (+ engaged toggle) into [`ActorAnimInputs`].
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
            // Locked/strafing gate is satisfied by passing the projection.
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
