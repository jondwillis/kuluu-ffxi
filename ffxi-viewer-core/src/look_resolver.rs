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
use crate::dat_vos2::LoadVos2Request;
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
/// Documented PC range is 1..=8; the formula extrapolates beyond
/// that and produces plausible file_ids for monstrosity / beastman
/// race codes seen in zone packets (e.g. `Kuu Mohzolhil` has
/// `race=29`). Whether those high-race lookups are *correct* is
/// empirical and not yet verified.
///
/// # Formula
///
/// Four tiers keyed on the low-12-bit id:
///
/// ```text
/// id   0..=255: file_id = 3680  + 256*slot + 3176*race + id
/// id 256..=319: file_id = 62555 +  64*slot +  448*race + id
/// id 320..=575: file_id = 69135 + 256*slot + 1536*race + id
/// id 576..=607: file_id = 98019 +  32*slot +  160*race + id
/// ```
///
/// # Returns
///
/// `None` when:
///   - slot or id is zero — the FFXI sentinel for "no item equipped"
///     in that slot
///   - race is zero — no PC ever has race 0
///   - id is > 607 — outside the formula's documented range
pub fn resolve_equipment_slot(slot_id: u16, race: u8) -> Option<u32> {
    let slot = u32::from((slot_id >> 12) & 0xF);
    let id = u32::from(slot_id & 0x0FFF);
    if slot == 0 || id == 0 || race == 0 {
        return None;
    }
    let race = u32::from(race);
    let file_id = match id {
        1..=255 => 3680 + 256 * slot + 3176 * race + id,
        256..=319 => 62555 + 64 * slot + 448 * race + id,
        320..=575 => 69135 + 256 * slot + 1536 * race + id,
        576..=607 => 98019 + 32 * slot + 160 * race + id,
        _ => return None,
    };
    Some(file_id)
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
    mut load_mmb_tx: MessageWriter<LoadMmbRequest>,
    mut load_vos2_tx: MessageWriter<LoadVos2Request>,
    mut commands: Commands,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };
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
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
            ..
        } = look.0
        {
            // Iterate slots in canonical order so a slot-by-slot
            // multi-mesh layout (Stage 4b) stays predictable.
            let slot_ids = [head, body, hands, legs, feet, main, sub, ranged];
            debug_assert_eq!(slot_ids.len(), EQUIP_SLOT_ORDER_LEN);
            let mut dispatched = 0;
            for slot in slot_ids {
                let Some(file_id) = resolve_equipment_slot(slot, race) else {
                    continue;
                };
                // VertexOs2 equipment files use chunk index 4 as the
                // primary skinned mesh — chunk[0] is the Rmp header,
                // chunk[1] is bone (Sk2), chunk[2] is the animation
                // (Mo2), chunk[3] is the low-LOD VertexOs2, chunk[4]
                // is the high-LOD VertexOs2, and chunks beyond hold
                // textures and additional LODs. This is empirically
                // consistent across the sample we have (file 13746,
                // Kuu Mohzolhil body) — a future fix may need to scan
                // chunks instead of indexing them statically.
                load_vos2_tx.write(LoadVos2Request {
                    file_id,
                    chunk_idx: 4,
                    entity_id: we.id,
                    race,
                    skeleton_file_id: None,
                });
                dispatched += 1;
            }
            if dispatched > 0 {
                info!(
                    "vos2 dispatch: entity_id={} race={} slots={}",
                    we.id, race, dispatched
                );
                if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
                    commands.entity(bevy_e).insert(EntityModel(look.0));
                }
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
        // animation library (MO2), and one-or-more body-part OS2s
        // all live inside this one DAT.
        let dat_id = npc_dat_id(modelid);
        let chunk_indices = crate::dat_vos2::enumerate_vos2_chunks(dat_id);
        if chunk_indices.is_empty() {
            // Formula picked a DAT with no OS2 — either the modelid
            // is out-of-range for the bucket boundaries, or this
            // entity uses a wrap container we don't yet support
            // (DOOR/TRANSPORT). Silent skip — `_zone_id` is reserved
            // for a future per-zone diagnostic toast.
            let _ = zone_id;
            continue;
        }
        debug_assert!(tracked.by_id.contains_key(&we.id));
        // Fire one VOS2 request per body-part chunk. The consumer
        // dedupes the per-frame skeleton load via the BAKED_SKELETONS
        // cache (`baked_skeleton_for_file`) so an N-chunk actor only
        // pays the SK2 parse cost once.
        for chunk_idx in &chunk_indices {
            load_vos2_tx.write(LoadVos2Request {
                file_id: dat_id,
                chunk_idx: *chunk_idx,
                entity_id: we.id,
                race: 0,
                skeleton_file_id: Some(dat_id),
            });
        }
        info!(
            "npc dispatch: entity_id={} modelid={} dat_id={} chunks={}",
            we.id, modelid, dat_id, chunk_indices.len()
        );
        if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
            commands.entity(bevy_e).insert(EntityModel(look.0));
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
        // head=0x1000 → slot=1, id=0 → empty (id 0 = unequipped sentinel)
        assert_eq!(resolve_equipment_slot(0x1000, 3), None);
        // body=0x2004 → slot=2, id=4, race 3 (Elvaan M) → tier 1
        // 3680 + 256*2 + 3176*3 + 4 = 13724
        assert_eq!(resolve_equipment_slot(0x2004, 3), Some(13724));
    }

    /// Race=0 and id=0 are both sentinels — the formula must reject
    /// them. Mirrors FFXI's "this slot has nothing in it" handling.
    #[test]
    fn equipment_sentinels_return_none() {
        assert_eq!(resolve_equipment_slot(0x0000, 3), None); // empty slot
        assert_eq!(resolve_equipment_slot(0x2000, 3), None); // slot set, id 0
        assert_eq!(resolve_equipment_slot(0x2004, 0), None); // race 0
    }

    /// Four-tier boundaries from the BlueGartr formula. Each tier
    /// has its own (base, slot-coeff, race-coeff) constants; this
    /// test pins exactly one sample per tier so a future "simplify"
    /// PR can't accidentally collapse them into one expression.
    #[test]
    fn equipment_formula_tier_boundaries() {
        // Tier 1: id 1..=255. Use slot=1, race=1, id=1 → 3680+256+3176+1 = 7113
        assert_eq!(resolve_equipment_slot(0x1001, 1), Some(7113));
        // Tier 2: id 256..=319. slot=1, race=1, id=256 → 62555+64+448+256 = 63323
        assert_eq!(resolve_equipment_slot(0x1100, 1), Some(63323));
        // Tier 3: id 320..=575. slot=1, race=1, id=320 → 69135+256+1536+320 = 71247
        assert_eq!(resolve_equipment_slot(0x1140, 1), Some(71247));
        // Tier 4: id 576..=607. slot=1, race=1, id=576 → 98019+32+160+576 = 98787
        assert_eq!(resolve_equipment_slot(0x1240, 1), Some(98787));
        // id 608 — outside formula's documented range.
        assert_eq!(resolve_equipment_slot(0x1260, 1), None);
    }

    /// Live look from a user `/look Kuu Mohzolhil` capture:
    /// race=29, body=0x2004. race 29 is outside the documented PC
    /// range (1..=8) — formula extrapolates and produces a plausible
    /// file_id, but visual correctness is unverified. This test pins
    /// the *output value* so we can spot any future formula
    /// refactor that drops support for high-race extrapolation.
    #[test]
    fn equipment_kuu_mohzolhil_body() {
        // body=0x2004, race=29 → tier 1: 3680 + 256*2 + 3176*29 + 4
        //   = 3680 + 512 + 92104 + 4 = 96300
        assert_eq!(resolve_equipment_slot(0x2004, 29), Some(96300));
    }
}
