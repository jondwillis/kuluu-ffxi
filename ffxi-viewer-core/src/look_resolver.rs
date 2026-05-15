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

/// Empirically-derived `(zone_id, modelid) -> (file_id, chunk_idx)`
/// mappings for `EntityLook::Standard`. Sorted by `(zone, modelid)`
/// for binary search; currently empty.
///
/// Why not a `HashMap`: at the expected scale (a few hundred entries
/// once populated), a sorted slice + `binary_search_by_key` is faster
/// than the hash + lookup overhead, and `const`-friendly so we pay
/// zero startup cost. Mirrors `zone_dat.rs`'s approach.
///
/// Why empirical instead of formula-driven: the background research
/// in `/private/tmp/.../a3e85bcf11a013ce5.output` confirmed that NPC
/// `Standard` modelids have no `BASE + modelid` formula — community
/// tools (AltanaViewer, DressUp, Stylist) all rely on hand-curated
/// per-NPC tables. The `npc-survey` example in `ffxi-dat/examples/`
/// is the workflow for filling this in.
const MODELID_TABLE: &[((u16, u16), (u32, usize))] = &[
    // ((zone_id, modelid), (file_id, chunk_idx))
    // — empty pending empirical survey; see module docs.
];

/// Resolve a fixed-NPC `Standard` look to its DAT location. Returns
/// `None` when the table has no entry for `(zone_id, modelid)`.
pub fn resolve_npc_standard(zone_id: u16, modelid: u16) -> Option<(u32, usize)> {
    MODELID_TABLE
        .binary_search_by_key(&(zone_id, modelid), |&(key, _)| key)
        .ok()
        .map(|i| MODELID_TABLE[i].1)
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

        let resolved = match look.0 {
            EntityLook::Standard { modelid } => resolve_npc_standard(zone_id, modelid),
            // Equipped handled above.
            EntityLook::Equipped { .. } => unreachable!(),
            // Doors / transports have their own resolvers (TODO):
            // they encode 'size' rather than 'modelid', so a separate
            // lookup keyed on size + zone goes here when sampled.
            EntityLook::Door { .. } | EntityLook::Transport { .. } => None,
        };
        let Some((file_id, chunk_idx)) = resolved else {
            continue;
        };

        // Need a confirmed Bevy entity so `LoadMmbRequest` consumer
        // can parent under it. The query already gave us
        // `WorldEntity`, but `LoadMmbRequest` carries the *wire* id —
        // the consumer does the `tracked.by_id` lookup itself, so we
        // just need to know the entity is present. (It is: the query
        // ran, which means the Bevy entity exists.)
        debug_assert!(tracked.by_id.contains_key(&we.id));
        load_mmb_tx.write(LoadMmbRequest {
            file_id,
            chunk_idx,
            world_pos: Vec3::ZERO,
            entity_id: Some(we.id),
            world_transform: None,
        });
        // Find the Bevy entity again to attach the signature marker.
        // The query iterator gives us &components, not the Bevy
        // Entity handle; we look it up via TrackedEntities (the same
        // path the consumer system uses).
        if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
            commands.entity(bevy_e).insert(EntityModel(look.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_npc_standard_returns_none_when_table_empty() {
        // No empirical mappings yet — every lookup should miss.
        assert_eq!(resolve_npc_standard(100, 1), None);
        assert_eq!(resolve_npc_standard(230, 42), None);
        assert_eq!(resolve_npc_standard(0, 0), None);
    }

    /// Once mappings start landing, this test guards the sort order
    /// that `binary_search_by_key` depends on. Empty table is
    /// trivially sorted; the assertion lives here so a future PR
    /// adding rows out-of-order trips this rather than silently
    /// returning wrong results.
    #[test]
    fn modelid_table_is_sorted() {
        for w in MODELID_TABLE.windows(2) {
            assert!(
                w[0].0 < w[1].0,
                "MODELID_TABLE must be sorted ascending by (zone, modelid)"
            );
        }
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
