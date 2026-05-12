//! MMB debug overlay: load a real FFXI entity model from a DAT file
//! (resolved via `ffxi-dat::DatRoot::from_env_or_default`) and spawn
//! it as a Bevy mesh at a chosen world position.
//!
//! The `_or_default` variant means the workspace `cargo run` path
//! works without anyone setting `FFXI_DAT_PATH` — it falls back to
//! `vendor/Game/SquareEnix/FINAL FANTASY XI`. Installed-binary users
//! still need to set the env var explicitly (the fallback resolves
//! relative to CWD, not relative to the executable).
//!
//! Plumbed via [`LoadMmbRequest`] events — the slash-command dispatcher
//! in `ffxi-client::view_native::text_input` fires the event; the
//! [`process_load_mmb_requests`] system consumes it. Keeps the text
//! input system from having to take direct asset-storage params.
//!
//! Native-only: `ffxi-dat` does synchronous `fs::read` of the user's
//! local install, which has no equivalent on wasm32. The browser viewer
//! can grow a parallel `LoadMmbHttp` path later if needed.
//!
//! See `ffxi-viewer-core/examples/mmb-view.rs` for a standalone
//! orbit-camera renderer of the same pipeline.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

use crate::snapshot::SceneState;

/// Marker for overlay entities spawned by this module — lets the
/// `/load_mmb clear` command (future work) find and despawn them.
#[derive(Component)]
pub struct MmbOverlay;

/// Spawn-an-MMB-at-position request. Fired by the slash-command
/// dispatcher; consumed by [`process_load_mmb_requests`].
///
/// `world_pos` is already in Bevy coordinates — the parser pre-applies
/// `ffxi_to_bevy` so this system stays unaware of the FFXI/Bevy axis
/// convention.
#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMmbRequest {
    pub file_id: u32,
    pub chunk_idx: usize,
    pub world_pos: Vec3,
}

/// Plugin: registers the MMB and MZB debug-overlay events and their
/// consumer systems. Added by `ViewerCorePlugin` so both front-ends
/// pick it up (the wasm cfg gate at the top of this file means the
/// whole module is absent on wasm32, and the lib.rs plugin add is
/// gated the same way).
pub struct DatOverlayPlugin;

impl Plugin for DatOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LoadMmbRequest>()
            .add_message::<crate::dat_mzb::LoadMzbRequest>()
            .init_resource::<crate::dat_mzb::LastAutoLoadedZone>()
            .init_resource::<crate::dat_mzb::DrawDistance>()
            .add_systems(
                Update,
                (
                    // Order matters: zone-change watcher writes a
                    // `LoadMzbRequest` that the consumer system reads
                    // *the same frame* — chaining the pair removes the
                    // one-frame delay that an unchained order would
                    // introduce on every zone transition.
                    crate::dat_mzb::auto_load_zone_geometry_system,
                    process_load_mmb_requests,
                    crate::dat_mzb::process_load_mzb_requests,
                )
                    .chain(),
            )
            // `/drawdistance setmob` consumer — culls non-PC entities
            // outside the configured radius. Stays decoupled from the
            // MZB load chain so it runs every frame independent of the
            // (rare) zone-change events.
            .add_systems(Update, crate::dat_mzb::cull_entities_by_distance);
            // Phase 1 `cull_mzb_by_distance` was removed: Phase 3 merged
            // everything into two entities anchored at world origin, so
            // the per-entity distance check would hide the whole zone
            // once the player walks >80 yalms from origin.
    }
}

/// Pure-data representation of one MMB sub-record, ready to bake into
/// a Bevy `Mesh`. Lives between [`load_mmb`] (parse) and the spawn
/// step so the parse half stays testable without an `App`.
pub struct MmbSubMesh {
    pub variant_name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// Load + decrypt + parse an MMB at the given file_id / chunk_idx.
/// Returns one [`MmbSubMesh`] per sub-record that has both vertices
/// and triangles. Sub-records that fail either check are skipped
/// silently — the caller sees how many came back and can warn the
/// operator if the count is zero.
///
/// All errors are flattened to `String` so the chat HUD can display
/// them. The underlying `ffxi_dat::DatError` already implements
/// `Display`, so the formatter does the work.
pub fn load_mmb(file_id: u32, chunk_idx: usize) -> Result<Vec<MmbSubMesh>, String> {
    let root = DatRoot::from_env_or_default()
        .map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = chunks
        .get(chunk_idx)
        .ok_or_else(|| format!("file has {} chunks, idx {chunk_idx} out of range", chunks.len()))?;
    if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
        return Err(format!(
            "chunk {chunk_idx} kind={:#x} ({:?}), not an MMB",
            chunk.kind,
            ChunkKind::label(chunk.kind),
        ));
    }

    let decrypted = mmb::decrypt(chunk.data).map_err(|e| format!("decrypt: {e}"))?;
    let header = MmbHeader::parse(&decrypted).map_err(|e| format!("header parse: {e}"))?;
    let subs = MmbSubRecord::find_all(header.payload);

    let mut out = Vec::with_capacity(subs.len());
    for sub in &subs {
        // Skip sub-records whose body can't fit a 36-byte vertex stride
        // for the declared count, or whose strip yields no triangles
        // after restart/winding decode. `mmb-view.rs` does the same
        // filter — keeps clod-style sub-records (Phase 8 work) from
        // dumping garbage geometry into the world.
        let Some(verts) = sub.parse_vertices() else { continue };
        let tris = sub.parse_triangle_list();
        if tris.is_empty() {
            continue;
        }
        let positions: Vec<[f32; 3]> = verts.iter().map(|v| v.pos).collect();
        let normals: Vec<[f32; 3]> = verts.iter().map(|v| v.normal).collect();
        let uvs: Vec<[f32; 2]> = verts.iter().map(|v| v.uv).collect();
        let colors: Vec<[f32; 4]> = verts
            .iter()
            .map(|v| {
                [
                    v.rgba[0] as f32 / 255.0,
                    v.rgba[1] as f32 / 255.0,
                    v.rgba[2] as f32 / 255.0,
                    v.rgba[3] as f32 / 255.0,
                ]
            })
            .collect();
        let indices: Vec<u32> = tris
            .iter()
            .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();
        out.push(MmbSubMesh {
            variant_name: sub.variant_name_str(),
            positions,
            normals,
            uvs,
            colors,
            indices,
        });
    }

    Ok(out)
}

/// Per-sub-record palette so multi-mesh models show their structure
/// against the scene. Same six-color cycle as `examples/mmb-view.rs`
/// for visual continuity.
const PALETTE: [Color; 6] = [
    Color::srgb(0.9, 0.4, 0.4),
    Color::srgb(0.4, 0.9, 0.4),
    Color::srgb(0.4, 0.4, 0.9),
    Color::srgb(0.9, 0.9, 0.4),
    Color::srgb(0.4, 0.9, 0.9),
    Color::srgb(0.9, 0.4, 0.9),
];

/// Consume [`LoadMmbRequest`] events: load the MMB and spawn one Bevy
/// mesh entity per sub-record under a parent transform at `world_pos`.
/// Failures get pushed into the scene's system chat buffer so the
/// operator sees why nothing showed up.
pub fn process_load_mmb_requests(
    mut events: MessageReader<LoadMmbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut scene_state: ResMut<SceneState>,
) {
    for req in events.read() {
        let submeshes = match load_mmb(req.file_id, req.chunk_idx) {
            Ok(s) => s,
            Err(msg) => {
                push_system_msg(
                    &mut scene_state,
                    format!("/load_mmb {} {}: {msg}", req.file_id, req.chunk_idx),
                );
                continue;
            }
        };
        if submeshes.is_empty() {
            push_system_msg(
                &mut scene_state,
                format!(
                    "/load_mmb {} {}: 0 renderable sub-records",
                    req.file_id, req.chunk_idx,
                ),
            );
            continue;
        }

        // Parent entity owns the world transform; sub-record meshes are
        // children at local-zero. Lets us despawn the whole model
        // recursively later (or animate it as one).
        let parent = commands
            .spawn((
                MmbOverlay,
                Transform::from_translation(req.world_pos),
                Visibility::default(),
            ))
            .id();

        let n_subs = submeshes.len();
        for (i, sub) in submeshes.into_iter().enumerate() {
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, sub.positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, sub.normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, sub.colors);
            mesh.insert_indices(Indices::U32(sub.indices));

            let mat = materials.add(StandardMaterial {
                base_color: PALETTE[i % PALETTE.len()],
                perceptual_roughness: 0.7,
                // FFXI triangle-strip winding isn't yet pinned to a
                // canonical front/back convention — render both sides
                // until Phase 8.5 settles winding (then flip back to
                // `Some(Face::Back)` for proper culling/lighting).
                cull_mode: None,
                ..default()
            });

            commands.spawn((
                MmbOverlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                Transform::default(),
                ChildOf(parent),
            ));
            let _ = sub.variant_name;
        }

        push_system_msg(
            &mut scene_state,
            format!(
                "/load_mmb {} {}: spawned {n_subs} sub-mesh{} at ({:.1}, {:.1}, {:.1})",
                req.file_id,
                req.chunk_idx,
                if n_subs == 1 { "" } else { "es" },
                req.world_pos.x,
                req.world_pos.y,
                req.world_pos.z,
            ),
        );
    }
}

fn push_system_msg(scene_state: &mut SceneState, text: String) {
    use ffxi_viewer_wire::{ChatChannel, ChatLine};
    // `push_local_toast`, not `snapshot.chat.push`: the snapshot's chat
    // buffer is server-owned and the next ingest tick overwrites it.
    // `local_toasts` persists across ticks until the cap evicts it.
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
    });
}
