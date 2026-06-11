//! Map an `EntityLook` to the DAT `(file_id, chunk_idx)` that holds
//! its MMB model. The output gets fed into a `LoadMmbRequest` which
//! the `dat_mmb` consumer spawns under the existing `WorldEntity`.
//!
//! # Why a resolver lives here
//!
//! `EntityLook` is a wire-side semantic value (race / equipment IDs,
//! or a fixed-NPC `modelid`). The DAT path the FFXI client would use
//! to render that look is a function of the install layout —
//! `ffxi-dat` knows where files live, but the *which file?* question
//! is FFXI-game-specific lookup data baked into the original client
//! and reverse-engineered by POLUtils. This module is the seam.
//!
//! # Current scope
//!
//! Only the `Standard { modelid }` variant has a resolver path here,
//! and its lookup table is **empty pending empirical derivation**.
//! See `MODELID_TABLE` below: the structure is in place so the
//! dispatch system (`process_entity_look_changes`) can fire
//! `LoadMmbRequest` events as soon as one mapping lands.
//!
//! `Equipped` is Stage 4 work and intentionally returns `None` here.
//! `Door` / `Transport` resolvers will follow the same shape as
//! `Standard` once we have a single confirmed sample.
//!
//! # How to add a mapping
//!
//! 1. Spawn into a zone where the test entity appears.
//! 2. Use the `/load_mmb_on <entity_id> <file_id> <chunk_idx>` debug
//!    command to find the `(file_id, chunk_idx)` that visually
//!    matches the entity (or run `ffxi-dat/examples/dat-mmb-survey`).
//! 3. Record `(zone_id, modelid)` from `/look <entity>` output.
//! 4. Add one row to `MODELID_TABLE` and a unit test confirming the
//!    lookup.

use bevy::prelude::*;
use ffxi_viewer_wire::EntityLook;

use crate::components::{EntityModel, LookComp, WorldEntity};
use crate::dat_mmb::LoadMmbRequest;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::TrackedEntities;
use crate::snapshot::SceneState;

/// FFXI equipment slot ordering as packed into `EntityLook::Equipped`.
/// The struct's field order is head/body/hands/legs/feet/main/sub/ranged.
/// We dispatch a `LoadVos2Request` for each non-empty slot.
const EQUIP_SLOT_ORDER_LEN: usize = 8;

/// Resolve a fixed-NPC `Standard` look to its actor DAT file_id.
///
/// FFXI's retail client uses a 4-bucket piecewise-linear formula to
/// map an NPC modelid to a ROM DAT file id. Reverse-engineered from
/// lotus-ffxi (`actor.cpp:24-35`); each bucket is a contiguous block
/// of file ids the retail install ships at the listed offsets.
///
/// | modelid       | dat_id formula                  |
/// |---------------|---------------------------------|
/// | < 1500        | `modelid + 1300`                |
/// | 1500..=2999   | `modelid - 1500 + 51795`        |
/// | 3000..=3499   | `modelid - 3000 + 99907`        |
/// | >= 3500       | `modelid - 3500 + 101739`       |
///
/// An earlier revision believed this required an empirical
/// (zone, modelid) lookup table (sourced from POLUtils-style
/// hand-curated dumps); cross-checking lotus-ffxi against the retail
/// client showed the table is just the formula's output. Use the
/// formula directly — no per-zone disambiguation needed.
pub fn npc_dat_id(modelid: u16) -> u32 {
    let m = modelid as u32;
    if m >= 3500 {
        m - 3500 + 101739
    } else if m >= 3000 {
        m - 3000 + 99907
    } else if m >= 1500 {
        m - 1500 + 51795
    } else {
        m + 1300
    }
}

/// Resolve one slot of an `EntityLook::Equipped` look to its DAT
/// file_id via the four-tier formula from the BlueGartr FFXI
/// reimagining thread (bluegartr.com/threads/131899), reverse-
/// engineered from packet captures of the retail client's DAT
/// requests when changing equipment.
///
/// # Inputs
///
/// `slot_id` is the raw u16 from `EntityLook::Equipped` (e.g.
/// `head=0x1000`, `body=0x2004`, `hands=0x3000`). FFXI packs the slot
/// in the high nibble and the item-model-id (0..607) in the low 12
/// bits — this function extracts both.
///
/// `race` is the FFXI race byte from `EntityLook::Equipped::race`.
/// Documented PC range is 1..=8. For races outside that range the
/// function returns `None`.
///
/// # Lookup
///
/// Uses lotus-ffxi's [`PCModelIDs`] piecewise-linear-band table
/// (one map per `(race, slot)`). For id `i` we find the largest
/// threshold `t ≤ i` in the breakpoint list, then return
/// `base + (i - t)` — or `None` when the chosen base is the
/// `0` sentinel (indicates "no DAT for this range").
///
/// The earlier closed-form formula (`3680 + 256*slot + 3176*race + id`)
/// produced correct file_ids only for race=1; other races
/// silently drifted by ~3000 IDs, loading the *wrong* DAT files
/// and rendering scrambled/missing geometry. See
/// `vendor/lotus-ffxi/ffxi/entity/actor_data.cppm:32-96` for the
/// authoritative source.
pub fn resolve_equipment_slot(slot_id: u16, race: u8) -> Option<u32> {
    let slot = u32::from((slot_id >> 12) & 0xF);
    let id = u32::from(slot_id & 0x0FFF);
    // id == 0 is NOT an early reject. In lotus's actor loader,
    // `GetPCModelDatID(modelid, race)` always looks up `upper_bound(0)`
    // and walks back one — which lands on `{0, base_dat}`, i.e. the
    // "naked" (no-equipment) model for that slot/race. Skipping id=0
    // here used to drop the head/scalp DAT when the user had no
    // helmet equipped (look.head = 0x1000 → slot=1, id=0), producing
    // a floating face mask with a visible gap to the body collar.
    // Weapon slots (main/sub/ranged) *do* legitimately render nothing
    // when unequipped, but the base-mesh table entries naturally
    // sentinel out (e.g. ranged slot id=0 maps to a placeholder DAT
    // we'll attempt to load and that may be empty — handled
    // downstream).
    if slot == 0 || slot > 8 || race == 0 || race > 8 {
        return None;
    }
    let bps = PC_MODEL_IDS.get((race - 1) as usize)?.get(slot as usize)?;
    // Linear scan — breakpoint lists are tiny (≤ 6 entries); a
    // binary search would be slower in practice. Find the largest
    // threshold ≤ id.
    let mut chosen: Option<(u16, u32)> = None;
    for &(thr, base) in *bps {
        if u32::from(thr) <= id {
            chosen = Some((thr, base));
        } else {
            break;
        }
    }
    let (thr, base) = chosen?;
    if base == 0 {
        return None;
    }
    Some(base + id - u32::from(thr))
}

/// Resolve a PC equipment slot from the **live wire shape**: an explicit
/// `slot_index` (1=head, 2=body, 3=hands, 4=legs, 5=feet, 6=main, 7=sub,
/// 8=ranged) plus a *pure* 12-bit `model_id` with no slot nibble.
///
/// This exists because the live look path strips the server's per-slot
/// `0xN000` tag: `ffxi_proto::decode`'s CHAR_PC decoder masks each equipment
/// field with `& 0x0FFF` (see `decode.rs`), so `EntityLook::Equipped::{head,
/// body,…}` reach us as bare model ids. Feeding those straight to
/// [`resolve_equipment_slot`] makes its `slot = slot_id >> 12` read `0` (the
/// face slot) and return `None` for every piece — the "remote PCs render
/// head-only" bug. Re-tagging the slot nibble here fixes it. The
/// `& 0x0FFF` mask keeps this idempotent for callers that still pass an
/// already-tagged value (the launcher / harness build slot-tagged u16s).
pub fn resolve_equipment_model(slot_index: u8, model_id: u16, race: u8) -> Option<u32> {
    if slot_index == 0 || slot_index > 8 {
        return None;
    }
    let slot_id = (u16::from(slot_index) << 12) | (model_id & 0x0FFF);
    resolve_equipment_slot(slot_id, race)
}

/// Transcribed from lotus's `PCModelIDs` table
/// (`vendor/lotus-ffxi/ffxi/entity/actor_data.cppm:32-96`).
/// Indexed `[race-1][slot]` where slot 0 = face (handled by
/// [`resolve_face`]), 1 = head, …, 8 = ranged. Each cell is a
/// sorted list of `(id_threshold, dat_base)` pairs; `dat_base=0`
/// marks the upper-bound sentinel.
///
/// Note that race 6 (Taru-F) shares slots 1..=8 with race 5
/// (Taru-M) per lotus — only the face slot differs.
type Breakpoints = &'static [(u16, u32)];
const PC_MODEL_IDS: [[Breakpoints; 9]; 8] = [
    // race 1 — Hume M
    [
        &[(0, 7080), (32, 0)],
        &[
            (0, 7112),
            (256, 63323),
            (320, 71247),
            (576, 98787),
            (608, 102961),
            (672, 0),
        ],
        &[
            (0, 7368),
            (256, 63387),
            (320, 71503),
            (576, 98819),
            (608, 103025),
            (672, 0),
        ],
        &[
            (0, 7624),
            (256, 63451),
            (320, 71759),
            (576, 98851),
            (608, 103089),
            (672, 0),
        ],
        &[
            (0, 7880),
            (256, 63515),
            (320, 72015),
            (576, 98883),
            (608, 103153),
            (672, 0),
        ],
        &[
            (0, 8136),
            (256, 63579),
            (320, 72271),
            (576, 98915),
            (608, 103217),
            (672, 0),
        ],
        &[
            (0, 8392),
            (512, 63643),
            (640, 72527),
            (896, 107301),
            (928, 0),
        ],
        &[
            (0, 41199),
            (512, 66459),
            (640, 81999),
            (896, 105201),
            (928, 0),
        ],
        &[(0, 9416), (256, 0)],
    ],
    // race 2 — Hume F
    [
        &[(0, 10256), (32, 0)],
        &[
            (0, 10288),
            (256, 63771),
            (320, 72783),
            (576, 98947),
            (608, 103281),
            (672, 0),
        ],
        &[
            (0, 10544),
            (256, 63835),
            (320, 73039),
            (576, 98979),
            (608, 103345),
            (672, 0),
        ],
        &[
            (0, 10800),
            (256, 63899),
            (320, 73295),
            (576, 99011),
            (608, 103409),
            (672, 0),
        ],
        &[
            (0, 11056),
            (256, 63963),
            (320, 73551),
            (576, 99043),
            (608, 103473),
            (672, 0),
        ],
        &[
            (0, 11312),
            (256, 64027),
            (320, 73807),
            (576, 99075),
            (608, 103537),
            (672, 0),
        ],
        &[
            (0, 11568),
            (512, 64091),
            (640, 74063),
            (896, 107601),
            (928, 0),
        ],
        &[
            (0, 42479),
            (512, 66587),
            (640, 82255),
            (896, 105501),
            (928, 0),
        ],
        &[(0, 12592), (256, 0)],
    ],
    // race 3 — Elvaan M
    [
        &[(0, 13432), (32, 0)],
        &[
            (0, 13464),
            (256, 64219),
            (320, 74319),
            (576, 99107),
            (608, 103601),
            (672, 0),
        ],
        &[
            (0, 13720),
            (256, 64283),
            (320, 74575),
            (576, 99139),
            (608, 103665),
            (672, 0),
        ],
        &[
            (0, 13976),
            (256, 64347),
            (320, 74831),
            (576, 99171),
            (608, 103729),
            (672, 0),
        ],
        &[
            (0, 14232),
            (256, 64411),
            (320, 75087),
            (576, 99203),
            (608, 103793),
            (672, 0),
        ],
        &[
            (0, 14488),
            (256, 64475),
            (320, 75343),
            (576, 99235),
            (608, 103857),
            (672, 0),
        ],
        &[
            (0, 14744),
            (512, 64539),
            (640, 75599),
            (896, 107901),
            (928, 0),
        ],
        &[
            (0, 43759),
            (512, 66715),
            (640, 82511),
            (896, 105801),
            (928, 0),
        ],
        &[(0, 15768), (256, 0)],
    ],
    // race 4 — Elvaan F
    [
        &[(0, 16608), (32, 0)],
        &[
            (0, 16640),
            (256, 64667),
            (320, 75855),
            (576, 99267),
            (608, 103921),
            (672, 0),
        ],
        &[
            (0, 16896),
            (256, 64731),
            (320, 76111),
            (576, 99299),
            (608, 103985),
            (672, 0),
        ],
        &[
            (0, 17152),
            (256, 64795),
            (320, 76367),
            (576, 99331),
            (608, 104049),
            (672, 0),
        ],
        &[
            (0, 17408),
            (256, 64859),
            (320, 76623),
            (576, 99363),
            (608, 104113),
            (672, 0),
        ],
        &[
            (0, 17664),
            (256, 64923),
            (320, 76879),
            (576, 99395),
            (608, 104177),
            (672, 0),
        ],
        &[
            (0, 17920),
            (512, 64987),
            (640, 77135),
            (896, 108201),
            (928, 0),
        ],
        &[
            (0, 45039),
            (512, 66843),
            (640, 82767),
            (896, 106101),
            (928, 0),
        ],
        &[(0, 18944), (256, 0)],
    ],
    // race 5 — Taru-M
    [
        &[(0, 19784), (32, 0)],
        &[
            (0, 19816),
            (256, 65115),
            (320, 77391),
            (576, 99427),
            (608, 104241),
            (672, 0),
        ],
        &[
            (0, 20072),
            (256, 65179),
            (320, 77647),
            (576, 99459),
            (608, 104305),
            (672, 0),
        ],
        &[
            (0, 20328),
            (256, 65243),
            (320, 77903),
            (576, 99491),
            (608, 104369),
            (672, 0),
        ],
        &[
            (0, 20584),
            (256, 65307),
            (320, 78159),
            (576, 99523),
            (608, 104433),
            (672, 0),
        ],
        &[
            (0, 20840),
            (256, 65371),
            (320, 78415),
            (576, 99555),
            (608, 104497),
            (672, 0),
        ],
        &[
            (0, 21096),
            (512, 65435),
            (640, 78671),
            (896, 108501),
            (928, 0),
        ],
        &[
            (0, 46319),
            (512, 66971),
            (640, 83023),
            (896, 106401),
            (928, 0),
        ],
        &[(0, 22120), (256, 0)],
    ],
    // race 6 — Taru-F (shares slots 1..=8 with race 5; face differs)
    [
        &[(0, 22960), (32, 0)],
        &[
            (0, 19816),
            (256, 65115),
            (320, 77391),
            (576, 99427),
            (608, 104241),
            (672, 0),
        ],
        &[
            (0, 20072),
            (256, 65179),
            (320, 77647),
            (576, 99459),
            (608, 104305),
            (672, 0),
        ],
        &[
            (0, 20328),
            (256, 65243),
            (320, 77903),
            (576, 99491),
            (608, 104369),
            (672, 0),
        ],
        &[
            (0, 20584),
            (256, 65307),
            (320, 78159),
            (576, 99523),
            (608, 104433),
            (672, 0),
        ],
        &[
            (0, 20840),
            (256, 65371),
            (320, 78415),
            (576, 99555),
            (608, 104497),
            (672, 0),
        ],
        &[
            (0, 21096),
            (512, 65435),
            (640, 78671),
            (896, 108501),
            (928, 0),
        ],
        &[
            (0, 46319),
            (512, 66971),
            (640, 83023),
            (896, 106401),
            (928, 0),
        ],
        &[(0, 22120), (256, 0)],
    ],
    // race 7 — Mithra
    [
        &[(0, 23184), (32, 0)],
        &[
            (0, 23216),
            (256, 65563),
            (320, 78927),
            (576, 99587),
            (608, 104561),
            (672, 0),
        ],
        &[
            (0, 23472),
            (256, 65627),
            (320, 79183),
            (576, 99619),
            (608, 104625),
            (672, 0),
        ],
        &[
            (0, 23728),
            (256, 65691),
            (320, 79439),
            (576, 99651),
            (608, 104689),
            (672, 0),
        ],
        &[
            (0, 23984),
            (256, 65755),
            (320, 79695),
            (576, 99683),
            (608, 104753),
            (672, 0),
        ],
        &[
            (0, 24240),
            (256, 65819),
            (320, 79951),
            (576, 99715),
            (608, 104817),
            (672, 0),
        ],
        &[
            (0, 24496),
            (512, 65883),
            (640, 80207),
            (896, 108801),
            (928, 0),
        ],
        &[
            (0, 47599),
            (512, 67099),
            (640, 83279),
            (896, 106701),
            (928, 0),
        ],
        &[(0, 25520), (256, 0)],
    ],
    // race 8 — Galka
    [
        &[(0, 26360), (32, 0)],
        &[
            (0, 26392),
            (256, 66011),
            (320, 80463),
            (576, 99747),
            (608, 104881),
            (672, 0),
        ],
        &[
            (0, 26648),
            (256, 66075),
            (320, 80719),
            (576, 99779),
            (608, 104945),
            (672, 0),
        ],
        &[
            (0, 26904),
            (256, 66139),
            (320, 80975),
            (576, 99811),
            (608, 105009),
            (672, 0),
        ],
        &[
            (0, 27160),
            (256, 66203),
            (320, 81231),
            (576, 99843),
            (608, 105073),
            (672, 0),
        ],
        &[
            (0, 27416),
            (256, 66267),
            (320, 81487),
            (576, 99875),
            (608, 105137),
            (672, 0),
        ],
        &[
            (0, 27672),
            (512, 66331),
            (640, 81743),
            (896, 109101),
            (928, 0),
        ],
        &[
            (0, 48879),
            (512, 67227),
            (640, 83535),
            (896, 107001),
            (928, 0),
        ],
        &[(0, 28696), (256, 0)],
    ],
];

/// Resolve a face DAT id from the wire `(face, race)` pair.
///
/// Mirrors lotus's `Actor::GetPCModelDatID(face-1, race)`, which
/// uses slot 0 of the `PCModelIDs` table. For each race the face
/// table has a single contiguous range `face∈[1..32] → base+(face-1)`,
/// where the per-race bases come from
/// `vendor/lotus-ffxi/ffxi/entity/actor_data.cppm:32-96`.
///
/// Note: races 5 and 6 (Taru-M and Taru-F) share the *skeleton* DAT
/// (19776) but have **different** face bases (19784 vs 22960) — they
/// share the rig but each has its own facial geometry.
pub fn resolve_face(face: u8, race: u8) -> Option<u32> {
    if race == 0 || race > 8 || face > 32 {
        return None;
    }
    // LSB defaults `char_look.face` to 0 when a character is created
    // without explicit face data (`vendor/server/sql/char_look.sql:15`).
    // Lotus treats face=0 as invalid; we fall back to face=1 so PCs
    // still render a face rather than an empty hood.
    let face = if face == 0 { 1 } else { face };
    const FACE_BASE: [u32; 8] = [7080, 10256, 13432, 16608, 19784, 22960, 23184, 26360];
    Some(FACE_BASE[(race - 1) as usize] + u32::from(face) - 1)
}

/// Look-driven MMB spawn dispatcher. Replaces the Stage 2 stub in
/// `scene::process_entity_look_changes` once this module is wired in
/// (see `lib.rs` schedule). For each entity whose `LookComp` changed
/// since last tick, look up the model and — if found — fire a
/// `LoadMmbRequest` parented to that entity. Marks the entity with
/// [`EntityModel`] so we don't respawn until the signature changes
/// again.
///
/// `Equipped` look is skipped entirely until Stage 4 lands.
pub fn dispatch_look_driven_models(
    state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    q_changed: Query<(&WorldEntity, &LookComp, Option<&EntityModel>), Changed<LookComp>>,
    load_mmb_tx: MessageWriter<LoadMmbRequest>,
    mut load_actor_tx: MessageWriter<crate::ffxi_actor_render::LoadActorRequest>,
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };
    // The faithful actor path is now the only character path; `settings` is
    // retained for any future Bevy-path toggle but no longer branches the
    // dispatch.
    let _ = &settings;
    for (we, look, current_model) in q_changed.iter() {
        // Already showing the right model? Skip.
        if let Some(EntityModel(sig)) = current_model {
            if *sig == look.0 {
                continue;
            }
        }

        // Equipped looks dispatch through a different format pipeline
        // (VertexOs2, kind 0x2A) — they spawn N requests in parallel,
        // one per non-empty slot. Standard / Door / Transport flow
        // through the MMB pipeline below.
        if let EntityLook::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
        } = look.0
        {
            // Gather the PC's equipment file_ids in the SAME order/logic the
            // legacy VOS2 path resolved them: face first (a raw face index
            // 1..=32, not a slot-encoded u16 — see `resolve_face`), then the
            // 8 equipment slots in canonical order. The faithful `load_pc`
            // loads the race skeleton DAT itself and skins each of these onto
            // it, so we hand it just the resolved file_ids.
            let mut equipment: Vec<u32> = Vec::new();
            if let Some(file_id) = resolve_face(face, race) {
                equipment.push(file_id);
            }
            // The wire model ids arrive WITHOUT the slot nibble (the CHAR_PC
            // decoder masks `& 0x0FFF`), so resolve each via the slot-indexed
            // entry point — passing the bare value to `resolve_equipment_slot`
            // would read slot 0 and drop every piece (the head-only bug).
            // Index 0 = head … 7 = ranged, i.e. slot_index = i + 1.
            let slot_models = [head, body, hands, legs, feet, main, sub, ranged];
            debug_assert_eq!(slot_models.len(), EQUIP_SLOT_ORDER_LEN);
            let mut slot_trace: [(u8, u16, Option<u32>); 8] = Default::default();
            for (i, &model_id) in slot_models.iter().enumerate() {
                let slot_index = (i + 1) as u8;
                let file_id = resolve_equipment_model(slot_index, model_id, race);
                slot_trace[i] = (slot_index, model_id, file_id);
                if let Some(file_id) = file_id {
                    equipment.push(file_id);
                }
            }
            // Any unresolved slot still means a missing model-id range in
            // `PC_MODEL_IDS`; log `(slot_index, model_id, file_id?)` so the
            // gap can be scoped from a live capture.
            if slot_trace.iter().any(|(_, _, r)| r.is_none()) {
                info!(
                    "pc equip unresolved (entity {} race {}): {:?}",
                    we.id, race, slot_trace
                );
            }

            // One faithful render request per entity (replaces the per-slot
            // VOS2 fan-out). `process_load_actor_requests` builds the rig.
            load_actor_tx.write(crate::ffxi_actor_render::LoadActorRequest {
                entity_id: we.id,
                subject: crate::ffxi_actor_render::ActorSubject::Pc {
                    race,
                    equipment: equipment.clone(),
                },
            });
            info!(
                "actor dispatch (pc): entity_id={} race={} equip={}",
                we.id,
                race,
                equipment.len()
            );
            if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
                // `try_insert`: actor may despawn between dispatch and flush.
                commands.entity(bevy_e).try_insert(EntityModel(look.0));
            }
            continue;
        }

        let modelid = match look.0 {
            EntityLook::Standard { modelid } => modelid,
            // Equipped handled above.
            EntityLook::Equipped { .. } => unreachable!(),
            // Doors / transports encode 'size' rather than 'modelid';
            // they need their own resolver path (TODO).
            EntityLook::Door { .. } | EntityLook::Transport { .. } => continue,
        };
        // Sentinel modelid 0 = "no model" — common for newly-spawned
        // entities awaiting a server-side look broadcast.
        if modelid == 0 {
            continue;
        }
        // NPC actor DAT (lotus-ffxi formula). The skeleton (SK2),
        // animation library (MO2), and one-or-more body meshes all live
        // inside this one DAT — the faithful `load_npc` collects them.
        let dat_id = npc_dat_id(modelid);
        if crate::dat_vos2::enumerate_vos2_chunks(dat_id).is_empty() {
            // Formula picked a DAT with no body geometry — either the
            // modelid is out-of-range for the bucket boundaries, or this
            // entity uses a wrap container we don't yet support
            // (DOOR/TRANSPORT). Silent skip — `zone_id` is reserved for a
            // future per-zone diagnostic toast.
            let _ = zone_id;
            continue;
        }
        debug_assert!(tracked.by_id.contains_key(&we.id));
        // One faithful render request per entity. `process_load_actor_requests`
        // builds the rig from this single DAT.
        load_actor_tx.write(crate::ffxi_actor_render::LoadActorRequest {
            entity_id: we.id,
            subject: crate::ffxi_actor_render::ActorSubject::Npc { file_id: dat_id },
        });
        info!(
            "actor dispatch (npc): entity_id={} modelid={} dat_id={}",
            we.id, modelid, dat_id
        );
        if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
            // `try_insert`: actor may despawn between dispatch and flush.
            commands.entity(bevy_e).try_insert(EntityModel(look.0));
        }
        // `load_mmb_tx` is still useful for door / transport models
        // once those resolvers exist — keep the param to avoid
        // re-plumbing later.
        let _ = &load_mmb_tx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each bucket's lower edge maps to the documented base file_id.
    /// Source: lotus-ffxi `actor.cpp:24-35`.
    #[test]
    fn npc_dat_id_bucket_lower_edges() {
        assert_eq!(npc_dat_id(0), 1300);
        assert_eq!(npc_dat_id(1500), 51795);
        assert_eq!(npc_dat_id(3000), 99907);
        assert_eq!(npc_dat_id(3500), 101739);
    }

    /// Bucket-boundary off-by-one guard: the formula is `>=` in each
    /// arm, so modelid=1499 stays in the first bucket and 1500 jumps.
    #[test]
    fn npc_dat_id_bucket_boundary_off_by_one() {
        assert_eq!(npc_dat_id(1499), 1499 + 1300);
        assert_eq!(npc_dat_id(1500), 51795);
        assert_eq!(npc_dat_id(2999), 2999 - 1500 + 51795);
        assert_eq!(npc_dat_id(3000), 99907);
    }

    /// Equipment slot extraction: high nibble = slot, low 12 bits =
    /// id. Documented FFXI packet layout.
    #[test]
    fn equipment_slot_extraction() {
        // head=0x1000 → slot=1, id=0 → race-3 head base DAT (Elvaan M
        // scalp/skull). Previously rejected as "unequipped"; lotus
        // treats id=0 as the *naked* (no helmet) mesh so the head
        // still renders under the face.
        assert_eq!(resolve_equipment_slot(0x1000, 3), Some(13464));
        // body=0x2004 → slot=2, id=4, race 3 (Elvaan M) → tier 1
        // base 13720 + (4 - 0) = 13724
        assert_eq!(resolve_equipment_slot(0x2004, 3), Some(13724));
    }

    /// The wire-shape entry point: a bare 12-bit model id (no slot nibble,
    /// as the CHAR_PC decoder delivers) must resolve via the slot index,
    /// equal to the slot-tagged call. The OLD path of feeding the bare value
    /// straight to `resolve_equipment_slot` reads slot 0 → `None` — the
    /// head-only bug this guards against.
    #[test]
    fn equipment_model_retags_bare_wire_ids() {
        // body slot (index 2), bare model id 4, race 3 → same as 0x2004.
        assert_eq!(resolve_equipment_model(2, 4, 3), Some(13724));
        // Feeding the bare id to the slot path reads slot 0 → None (the bug).
        assert_eq!(resolve_equipment_slot(4, 3), None);
        // Idempotent for an already-tagged value: the inner mask strips the
        // stray nibble before re-tagging, so 0x2004 still lands on slot 2.
        assert_eq!(resolve_equipment_model(2, 0x2004, 3), Some(13724));
        // Out-of-range slot index → None (slot 0 is the face).
        assert_eq!(resolve_equipment_model(0, 4, 3), None);
        assert_eq!(resolve_equipment_model(9, 4, 3), None);
    }

    /// Slot=0 and race=0 are hard sentinels — face slot is handled
    /// by [`resolve_face`], not this function, and race=0 is invalid.
    #[test]
    fn equipment_sentinels_return_none() {
        assert_eq!(resolve_equipment_slot(0x0000, 3), None); // slot 0 = face, handled separately
        assert_eq!(resolve_equipment_slot(0x2004, 0), None); // race 0
        assert_eq!(resolve_equipment_slot(0x2000, 3), Some(13720)); // slot 2 id 0 = naked body
    }

    /// Four-tier boundaries from the BlueGartr formula. Each tier
    /// has its own (base, slot-coeff, race-coeff) constants; this
    /// Pin one value per breakpoint band of lotus's PCModelIDs
    /// (race 1, slot 1 = head). Catches any future regression in
    /// the table or the lookup logic.
    #[test]
    fn equipment_table_band_samples_race1_head() {
        // Band [0..256): base 7112 → id=1 returns 7113.
        assert_eq!(resolve_equipment_slot(0x1001, 1), Some(7113));
        // Band [256..320): base 63323 → id=256 returns 63323.
        assert_eq!(resolve_equipment_slot(0x1100, 1), Some(63323));
        // Band [320..576): base 71247 → id=320 returns 71247.
        assert_eq!(resolve_equipment_slot(0x1140, 1), Some(71247));
        // Band [576..608): base 98787 → id=576 returns 98787.
        assert_eq!(resolve_equipment_slot(0x1240, 1), Some(98787));
        // Band [608..672): base 102961 → id=608 returns 102961.
        // Lotus's table extends here; the prior closed-form formula
        // rejected id ≥ 608.
        assert_eq!(resolve_equipment_slot(0x1260, 1), Some(102961));
        // Band [672..): sentinel 0 → None.
        assert_eq!(resolve_equipment_slot(0x12A0, 1), None);
    }

    /// Non-Hume races used to silently misroute through the old
    /// linear formula. Pin two known-good values from lotus's table
    /// to lock the per-race correctness.
    #[test]
    fn equipment_per_race_correctness() {
        // Race 8 (Galka), slot 2 (body), id=8. Lotus table:
        // breakpoint (0, 26648) → 26648 + 8 = 26656. The old formula
        // returned 29608 (off by 2952), causing Galka to render
        // wrong/missing geometry.
        assert_eq!(resolve_equipment_slot(0x2008, 8), Some(26656));
        // Race 7 (Mithra), slot 4 (legs), id=4. Lotus table:
        // (0, 23984) → 23984 + 4 = 23988.
        assert_eq!(resolve_equipment_slot(0x4004, 7), Some(23988));
    }

    /// Out-of-range race byte (e.g. monstrosity / beastman) now
    /// returns None rather than extrapolating garbage. The earlier
    /// formula generated arbitrary file_ids for race≥9 that didn't
    /// correspond to real DATs. Beastman-NPC rendering should go
    /// through the NPC modelid path (`npc_dat_id`), not this one.
    #[test]
    fn equipment_rejects_high_race_codes() {
        assert_eq!(resolve_equipment_slot(0x2004, 29), None);
    }
}
