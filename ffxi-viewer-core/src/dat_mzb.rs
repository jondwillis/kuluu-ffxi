//! MZB debug overlay: load a real FFXI zone mesh-library from a DAT
//! file and spawn every mesh in the library at a chosen world position.
//!
//! Pattern mirrors [`crate::dat_mmb`] — slash-command dispatcher fires
//! [`LoadMzbRequest`], [`process_load_mzb_requests`] consumes it. The
//! mesh-library geometry sits near the origin in MZB's own coordinate
//! frame; spatial-index decode (grid/quadtree/placement transforms)
//! lives in Phase 9b. Without those, the overlay shows mesh data as
//! though every mesh is at the library origin — useful for visual
//! validation of the parser even before placement is wired.
//!
//! Native-only for the same reason as `dat_mmb.rs`: `ffxi-dat::DatRoot`
//! does sync `fs::read` of the user's local install.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

use crate::snapshot::SceneState;

/// Marker for overlay entities spawned by this module. Includes both
/// `/load_mzb`-loaded and auto-loaded-on-zone-change entities — the
/// finer-grained [`AutoMzbOverlay`] marker is added in *addition* on
/// auto-loaded ones so the zone-change watcher can despawn them
/// without clobbering the operator's manual loads.
#[derive(Component)]
pub struct MzbOverlay;

/// Sub-marker added on top of [`MzbOverlay`] when the entity was
/// spawned by the auto-load-on-zone-change system. Lets that system
/// recognize "its own" entities for despawn-on-next-zone, leaving
/// `/load_mzb` manual loads alone.
#[derive(Component)]
pub struct AutoMzbOverlay;

/// Spawn-a-zone-mesh-library-at-position request. `world_pos` is
/// already in Bevy coordinates — the parser pre-applies `ffxi_to_bevy`
/// so this system stays axis-agnostic.
#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMzbRequest {
    pub file_id: u32,
    /// Optional explicit chunk index. `None` means "scan for the first
    /// kind=0x1C (MZB) chunk in the file", matching the convenience
    /// behavior of `examples/dat-mzb-probe.rs`. Zone-bundle DATs
    /// usually have exactly one MZB.
    pub chunk_idx: Option<usize>,
    pub world_pos: Vec3,
    /// `true` for auto-load-on-zone-change requests — the spawn code
    /// tags the resulting entities with [`AutoMzbOverlay`] so the
    /// zone-change watcher can identify them on the next change.
    /// `/load_mzb` slash command always sets this `false`.
    pub auto_loaded: bool,
}

/// Pure-data Bevy-ready bake of one MZB library mesh.
pub struct MzbSubMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// Per-mesh flag from the MZB record header. Bit 0 = does NOT
    /// block LoS (visual-only / non-collision). Surface so the caller
    /// can colorize collision vs non-collision geometry distinctly.
    pub flags: u16,
}

/// Load + decrypt + parse all meshes in the first (or specified) MZB
/// chunk of `file_id`. Returns ready-to-bake submeshes.
pub fn load_mzb(file_id: u32, chunk_idx: Option<usize>) -> Result<Vec<MzbSubMesh>, String> {
    let root = DatRoot::from_env_or_default()
        .map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let (idx, chunk) = match chunk_idx {
        Some(i) => (
            i,
            chunks
                .get(i)
                .ok_or_else(|| format!("chunk_idx {i} out of range ({} chunks)", chunks.len()))?,
        ),
        None => chunks
            .iter()
            .enumerate()
            .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
            .ok_or_else(|| {
                format!(
                    "no MZB (kind 0x1C) chunk in file_id {file_id} ({} chunks)",
                    chunks.len()
                )
            })?,
    };
    if chunk.kind != ChunkKind::Mzb as u8 {
        return Err(format!(
            "chunk[{idx}] kind=0x{:02X} ({:?}), not an MZB",
            chunk.kind,
            ChunkKind::label(chunk.kind),
        ));
    }

    let (_header, meshes) =
        mzb::parse_all(chunk.data).map_err(|e| format!("MZB parse_all: {e}"))?;

    let mut out = Vec::with_capacity(meshes.len());
    for m in meshes {
        if m.vertices.is_empty() || m.triangles.is_empty() {
            continue;
        }
        let positions: Vec<[f32; 3]> = m.vertices.iter().map(|v| v.pos).collect();
        let indices: Vec<u32> = m
            .triangles
            .iter()
            .flat_map(|t| [t[0], t[1], t[2]])
            .collect();
        out.push(MzbSubMesh {
            positions,
            indices,
            flags: m.flags,
        });
    }
    Ok(out)
}

/// Spawn each MZB submesh as its own child entity under a parent
/// transform at `world_pos`. Collision and non-collision meshes are
/// distinct colors so the operator can see which geometry actually
/// participates in LoS / pathing.
///
/// MZB carries vertex positions only — no normals per vertex. We let
/// Bevy compute flat normals from positions for shading. Collision
/// (flags bit 0 cleared) and non-collision (flags bit 0 set) get
/// different palettes so they're visually distinguishable when
/// stacked at the same origin.
pub fn process_load_mzb_requests(
    mut events: MessageReader<LoadMzbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut scene_state: ResMut<SceneState>,
) {
    for req in events.read() {
        let submeshes = match load_mzb(req.file_id, req.chunk_idx) {
            Ok(s) => s,
            Err(msg) => {
                push_system_msg(
                    &mut scene_state,
                    format!("/load_mzb {}: {msg}", req.file_id),
                );
                continue;
            }
        };
        if submeshes.is_empty() {
            push_system_msg(
                &mut scene_state,
                format!("/load_mzb {}: 0 renderable meshes", req.file_id),
            );
            continue;
        }

        let n_total = submeshes.len();
        let n_collision = submeshes.iter().filter(|s| s.flags & 1 == 0).count();

        let collision_mat = materials.add(StandardMaterial {
            // Muted teal: reads as "solid ground" against the dark scene.
            base_color: Color::srgb(0.30, 0.65, 0.65),
            perceptual_roughness: 0.85,
            cull_mode: None,
            ..default()
        });
        let noncollision_mat = materials.add(StandardMaterial {
            // Translucent orange: visual-only walls/decoration. Bit 0 of
            // the MZB flags == "does NOT block LoS" — operator sees these
            // shouldn't matter for pathing.
            base_color: Color::srgba(0.85, 0.55, 0.20, 0.45),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.7,
            cull_mode: None,
            ..default()
        });

        let mut parent_spawn = commands.spawn((
            MzbOverlay,
            Transform::from_translation(req.world_pos),
            Visibility::default(),
        ));
        if req.auto_loaded {
            parent_spawn.insert(AutoMzbOverlay);
        }
        let parent = parent_spawn.id();

        for sub in submeshes {
            let mut mesh =
                Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, sub.positions);
            mesh.insert_indices(Indices::U32(sub.indices));
            // MZB stores per-triangle normals, not per-vertex; let Bevy
            // bake flat normals from positions so PBR lighting still works.
            mesh.compute_flat_normals();
            let mat = if sub.flags & 1 == 0 {
                collision_mat.clone()
            } else {
                noncollision_mat.clone()
            };
            let mut child = commands.spawn((
                MzbOverlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                Transform::default(),
                ChildOf(parent),
            ));
            if req.auto_loaded {
                child.insert(AutoMzbOverlay);
            }
        }

        push_system_msg(
            &mut scene_state,
            format!(
                "/load_mzb {}: {n_total} meshes ({n_collision} collision, {} non-collision) at ({:.1}, {:.1}, {:.1})",
                req.file_id,
                n_total - n_collision,
                req.world_pos.x,
                req.world_pos.y,
                req.world_pos.z,
            ),
        );
    }
}

/// Last zone_id the auto-load watcher fired for. `None` until the
/// player first zones in. Tracked separately from the snapshot so we
/// can detect transitions without depending on Bevy's `Res<...>`
/// change-detection (which would fire on every snapshot replacement
/// regardless of whether zone_id actually changed).
#[derive(Resource, Default)]
pub struct LastAutoLoadedZone {
    pub zone_id: Option<u16>,
}

/// Watch [`SceneState::snapshot::zone_id`] for changes. On every
/// transition (None → Some, Some(A) → Some(B), Some(A) → None):
///   1. Despawn every entity tagged [`AutoMzbOverlay`] (preserving the
///      operator's manual `/load_mzb` loads).
///   2. If the new zone has a known DAT file_id mapping, fire a
///      [`LoadMzbRequest`] at FFXI world origin with
///      `auto_loaded: true`.
///
/// Zones without a known mapping fall through quietly — the previous
/// zone's auto-load is still despawned (so we don't leave stale
/// geometry from zone A floating in zone B), but no new request is
/// fired. The chat HUD gets a one-line note so the operator can tell
/// the difference between "mapping missing" and "auto-load broken".
pub fn auto_load_zone_geometry_system(
    mut scene_state: ResMut<SceneState>,
    mut last: ResMut<LastAutoLoadedZone>,
    mut commands: Commands,
    mut load_tx: MessageWriter<LoadMzbRequest>,
    auto_q: Query<Entity, With<AutoMzbOverlay>>,
) {
    let current = scene_state.snapshot.zone_id;
    if current == last.zone_id {
        return;
    }
    // Transition detected — despawn previous auto-load even if we
    // don't end up firing a new one (covers the Some(A) → None
    // "logout / charselect" case).
    for e in auto_q.iter() {
        commands.entity(e).despawn();
    }
    last.zone_id = current;
    let Some(zone_id) = current else { return };

    match ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) {
        Some(file_id) => {
            // FFXI world origin = Bevy origin: `ffxi_to_bevy(0,0,0)`
            // = `Vec3::ZERO`. MZB vertex data is already in zone-local
            // space (which IS the zone's coordinate frame).
            load_tx.write(LoadMzbRequest {
                file_id,
                chunk_idx: None,
                world_pos: Vec3::ZERO,
                auto_loaded: true,
            });
            // Don't push a chat line here — `process_load_mzb_requests`
            // already pushes one when the actual spawn lands. Doubling
            // the message just to say "we asked to load it" is noise.
        }
        None => {
            push_system_msg(
                &mut scene_state,
                format!("auto-load: no DAT mapping for zone {zone_id} (Phase 11b table pending)"),
            );
        }
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
