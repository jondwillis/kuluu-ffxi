//! Sub-areas: the building interiors (shops, guilds, houses) a town zone swaps in
//! place of its closed-up exterior shells without a server-side zone change.
//!
//! Retail keeps two zone blocks live at once
//! (research/XIClient/src/XIClient/include/Rendering/ZoneRenderer.h ZoneRenderer MAX_ZONE_LOAD_COUNT,
//! `MAX_ZONE_LOAD_COUNT = 2`): the main zone plus whichever sub-area the player
//! stands in, loaded by DAT index in `ZoneRenderer::PreDraw`
//! (ZoneRenderer.cpp ZoneRenderer::PreDraw). While that block is up, `SetRenderTypes`
//! (ZoneRenderer.cpp ZoneRenderer::SetRenderTypes) demotes every placement whose
//! [`MmbPlacement::sub_area_link`] equals the active id to a RenderType the draw
//! passes skip, and the collision manager drops the matching collision objects
//! (CollisionManager.cpp CollisionManager::KO_CharaCollision) — the shell disappears in both senses.

use crate::mzb::MmbPlacement;
use crate::zone_interaction::{self, ZoneInteraction};
use crate::Result;

/// ZoneRenderer.cpp ZoneRenderer::PreDraw — the sub-area's DAT index is its id put through the
/// same file-table offsets `LoadZoneFile` (ZoneRenderer.cpp ZoneRenderer::LoadZoneFile) applies to a
/// zone id. research/cexi-docs/zone/subareas.md "3. Resolving the interior DAT" states the same pair.
pub const SUB_AREA_FILE_ID_OFFSET: u32 = 0x64;
pub const SUB_AREA_FILE_ID_OFFSET_HIGH: u32 = 0x1_44F7;

/// Which offset applies. cexi-docs subareas.md "3. Resolving the interior DAT" puts the split here; XIClient's
/// decompiled predicate (`>= 700 || >= 600`, ZoneRenderer.cpp ZoneRenderer::PreDraw) is a collapsed
/// two-way compare and so is not bit-level authority. The retail install narrows
/// the boundary to `(0x24E, 0x271]` — of the 280 sub-area ids its zone DATs
/// declare, every one of the 264 at or below `0x24E` resolves to an MZB-carrying
/// DAT under [`SUB_AREA_FILE_ID_OFFSET`] and none does above, while all 16 from
/// `0x271` up resolve only under [`SUB_AREA_FILE_ID_OFFSET_HIGH`]. No shipped id
/// falls in the ambiguous band, so either reading picks the same file.
pub const SUB_AREA_ID_HIGH_MIN: u32 = 0x271;

/// VTABLE/FTABLE file id of a sub-area's interior DAT. The interior is a
/// self-contained mini-zone already expressed in the parent zone's world space.
pub fn sub_area_file_id(sub_area_id: u32) -> u32 {
    if sub_area_id >= SUB_AREA_ID_HIGH_MIN {
        sub_area_id + SUB_AREA_FILE_ID_OFFSET_HIGH
    } else {
        sub_area_id + SUB_AREA_FILE_ID_OFFSET
    }
}

/// One interior a zone can swap in, with the trigger volumes that select it.
#[derive(Debug, Clone, PartialEq)]
pub struct SubArea {
    pub id: u32,
    pub file_id: u32,
    /// Every `m`-rect declaring this id; a building with several doorways
    /// declares one per doorway.
    pub triggers: Vec<ZoneInteraction>,
}

impl SubArea {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.triggers.iter().any(|t| t.contains(p))
    }
}

/// The zone's sub-areas, ascending by id.
pub fn from_interactions(interactions: &[ZoneInteraction]) -> Vec<SubArea> {
    let mut out: Vec<SubArea> = Vec::new();
    for i in interactions {
        let Some(id) = i.sub_area_id() else { continue };
        match out.binary_search_by_key(&id, |s| s.id) {
            Ok(at) => out[at].triggers.push(*i),
            Err(at) => out.insert(
                at,
                SubArea {
                    id,
                    file_id: sub_area_file_id(id),
                    triggers: vec![*i],
                },
            ),
        }
    }
    out
}

/// The zone's sub-areas read straight from its zone resource DAT.
pub fn from_dat(bytes: &[u8]) -> Result<Vec<SubArea>> {
    Ok(from_interactions(&zone_interaction::from_dat(bytes)?))
}

/// World-space AABB of the exterior shell a sub-area stands in for: the union of
/// the placements whose [`MmbPlacement::sub_area_link`] names it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubAreaShell {
    pub id: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl SubAreaShell {
    pub fn contains_inflated(&self, p: [f32; 3], margin: f32) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] - margin && p[i] <= self.max[i] + margin)
    }
}

/// How far past a shell the player must get before the latch drops its interior.
/// The doorway rects stand off from the shell they open — Port Jeuno's measured
/// worst case is 5.0-5.6 units of clearance — so a tighter test would drop the
/// interior while the player is still in the doorway.
pub const SUB_AREA_SHELL_CLEAR_MARGIN: f32 = 8.0;

/// Retail's `CollisionMng.field_4` (`-1` = outdoors, ZoneRenderer.cpp ZoneRenderer::ZoneRenderer) kept as
/// state over a stream of player positions, because it cannot be a pure function of
/// position: of the 334 sub-area triggers the shipped zones declare, none lies
/// inside its own interior (205 straddle it, 129 sit wholly outside), so a
/// containment test would drop the interior as the player crossed the doorway and
/// rematerialise the shell around them.
///
/// The SET rule — latch on the rising edge into a trigger — is inference by
/// elimination rather than code-cited: XIClient has no writer of
/// `CollisionMng.field_4` besides two `= -1` resets, and the bridging
/// `gcZoneSubMapChangeSet` is an `XICLIENT_CODE_MISSING` stub. The clear condition
/// is unsettled; this is the conservative fail-safe, keeping the interior until the
/// player is clear of every trigger *and* of [`SubAreaShell`] by
/// [`SUB_AREA_SHELL_CLEAR_MARGIN`].
#[derive(Debug, Clone, PartialEq)]
pub struct SubAreaLatch {
    triggers: Vec<ZoneInteraction>,
    shells: Vec<SubAreaShell>,
    inside: Vec<bool>,
    active: Option<u32>,
}

impl SubAreaLatch {
    /// `shells` may omit ids; a sub-area with no shell is never cleared
    /// geometrically, only by a `param == 0` trigger.
    pub fn new(interactions: &[ZoneInteraction], shells: Vec<SubAreaShell>) -> Self {
        let triggers: Vec<ZoneInteraction> = interactions
            .iter()
            .filter(|i| i.is_sub_area_trigger())
            .copied()
            .collect();
        Self {
            inside: vec![false; triggers.len()],
            triggers,
            shells,
            active: None,
        }
    }

    pub fn active(&self) -> Option<u32> {
        self.active
    }

    pub fn shells(&self) -> &[SubAreaShell] {
        &self.shells
    }

    pub fn triggers(&self) -> &[ZoneInteraction] {
        &self.triggers
    }

    /// Drops the interior and forgets which rects the player stood in, for a zone
    /// change or a warp — retail's `field_4 = -1` reset.
    pub fn reset(&mut self) {
        self.active = None;
        self.inside.iter_mut().for_each(|b| *b = false);
    }

    /// Puts the latch straight into `active` without a trigger crossing, for the
    /// `SubMapNumber` the server hands over at zone-in: a character who logged
    /// out inside a shop is standing in the interior before they ever cross its
    /// doorway rect, and the rising-edge rule alone can only see them leave.
    /// From here the ordinary clear rule applies, so walking out still drops it.
    pub fn set_active(&mut self, id: Option<u32>) {
        self.active = id;
    }

    /// Advance the latch to player position `p` (FFXI zone space) and report the
    /// sub-area now swapped in. Feed the answer to [`crate::mzb::drawn_placements`]
    /// and [`crate::mzb::MzbPlacement::collides_in`].
    pub fn update(&mut self, p: [f32; 3]) -> Option<u32> {
        let mut entered: Option<u32> = None;
        let mut any_inside = false;
        for (i, t) in self.triggers.iter().enumerate() {
            let now = t.contains(p);
            if now && !self.inside[i] {
                entered = Some(t.param);
            }
            self.inside[i] = now;
            any_inside |= now;
        }

        if let Some(param) = entered {
            self.active = (param != 0).then_some(param);
        } else if let Some(id) = self.active {
            let held_by_shell = self
                .shells
                .iter()
                .find(|s| s.id == id)
                .is_none_or(|s| s.contains_inflated(p, SUB_AREA_SHELL_CLEAR_MARGIN));
            if !any_inside && !held_by_shell {
                self.active = None;
            }
        }
        self.active
    }
}

/// Sub-area links the zone's placements name but no `m`-rect declares. Retail can
/// never activate these, so their placeholder shells are permanently visible —
/// a real, if small, gap rather than a parse failure, and worth a log line at the
/// call site instead of a silent skip.
pub fn undeclared_placeholder_links(
    sub_areas: &[SubArea],
    placements: &[MmbPlacement],
) -> Vec<u32> {
    let mut out: Vec<u32> = placements
        .iter()
        .map(|p| p.sub_area_link)
        .filter(|id| *id != 0 && sub_areas.binary_search_by_key(id, |s| s.id).is_err())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datid::DatId;

    fn trigger(id: u32, position: [f32; 3], orientation_y: f32, size: [f32; 3]) -> ZoneInteraction {
        ZoneInteraction {
            position,
            rect_class: zone_interaction::RECT_CLASS_HIT_CHECKED,
            orientation: [0.0, orientation_y, 0.0],
            size,
            source_id: DatId(*b"m6t1"),
            dest_id: Some(DatId([0x20, 0, 0, 0])),
            param: id,
            terrain_flags: 0,
            map_id: 0,
            elevator_bottom_y: 0.0,
            elevator_top_y: 0.0,
        }
    }

    #[test]
    fn file_id_uses_the_low_offset_below_the_split() {
        assert_eq!(sub_area_file_id(0x1C6), 0x22A);
        assert_eq!(sub_area_file_id(0x1D2), 0x236);
        assert_eq!(sub_area_file_id(SUB_AREA_ID_HIGH_MIN - 1), 0x2D4);
    }

    #[test]
    fn file_id_uses_the_high_offset_at_and_above_the_split() {
        assert_eq!(sub_area_file_id(SUB_AREA_ID_HIGH_MIN), 83816);
        assert_eq!(sub_area_file_id(0x280), 83831);
    }

    #[test]
    fn only_m_rects_with_a_dest_and_a_param_are_sub_areas() {
        let mut no_dest = trigger(0x1C6, [0.0; 3], 0.0, [1.0; 3]);
        no_dest.dest_id = None;
        let leave = trigger(0, [0.0; 3], 0.0, [1.0; 3]);
        let mut zone_line = trigger(0x1C6, [0.0; 3], 0.0, [1.0; 3]);
        zone_line.source_id = DatId(*b"zmr0");
        let mut region = trigger(0x1C6, [0.0; 3], 0.0, [1.0; 3]);
        region.rect_class = 555;

        assert_eq!(no_dest.sub_area_id(), None);
        assert_eq!(leave.sub_area_id(), None);
        assert_eq!(zone_line.sub_area_id(), None);
        assert_eq!(region.sub_area_id(), None);
        assert!(region.is_sub_map_region());
        assert_eq!(
            trigger(0x1C6, [0.0; 3], 0.0, [1.0; 3]).sub_area_id(),
            Some(0x1C6)
        );
    }

    /// The leave signal is a `param == 0` trigger, which `sub_area_id` folds into
    /// the same `None` as "not a trigger" — only [`ZoneInteraction::sub_area_param`]
    /// keeps them apart, and the latch depends on that.
    #[test]
    fn a_zero_param_trigger_is_a_leave_signal_not_a_non_trigger() {
        let leave = trigger(0, [0.0; 3], 0.0, [1.0; 3]);
        let mut zone_line = trigger(0, [0.0; 3], 0.0, [1.0; 3]);
        zone_line.source_id = DatId(*b"zmr0");

        assert_eq!(leave.sub_area_param(), Some(0));
        assert_eq!(zone_line.sub_area_param(), None);
        assert_eq!(from_interactions(&[leave]), Vec::new());
    }

    #[test]
    fn triggers_group_by_id_ascending() {
        let all = [
            trigger(0x1C8, [0.0; 3], 0.0, [1.0; 3]),
            trigger(0x1C6, [10.0, 0.0, 0.0], 0.0, [1.0; 3]),
            trigger(0x1C8, [20.0, 0.0, 0.0], 0.0, [1.0; 3]),
        ];
        let subs = from_interactions(&all);
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].id, 0x1C6);
        assert_eq!(subs[0].triggers.len(), 1);
        assert_eq!(subs[1].id, 0x1C8);
        assert_eq!(subs[1].triggers.len(), 2);
        assert_eq!(subs[1].file_id, sub_area_file_id(0x1C8));
    }

    #[test]
    fn size_is_the_full_extent_and_the_box_is_centered() {
        let t = trigger(0x1C6, [100.0, -5.0, 50.0], 0.0, [4.0, 8.0, 2.0]);
        assert!(t.contains([100.0, -5.0, 50.0]));
        assert!(t.contains([101.9, -1.1, 50.9]));
        assert!(t.contains([98.1, -8.9, 49.1]));
        assert!(!t.contains([102.1, -5.0, 50.0]), "past half of size.x");
        assert!(!t.contains([100.0, -0.9, 50.0]), "past half of size.y");
        assert!(!t.contains([100.0, -5.0, 51.1]), "past half of size.z");
    }

    /// Pins the yaw convention: RidManager.cpp RidManager::Add rotates by `-orientation.y`
    /// with XIClient's row-vector `RotateY` (Matrix4.cpp Matrix4::RotateY), so the box's
    /// local +x runs along `(cos y, 0, -sin y)` in zone space.
    #[test]
    fn yaw_rotates_the_box_the_way_retail_does() {
        let quarter = std::f32::consts::FRAC_PI_2;
        let t = trigger(0x1C6, [0.0; 3], quarter, [8.0, 100.0, 2.0]);
        assert!(
            t.contains([0.0, 0.0, -3.0]),
            "the long axis swung onto -z at +90 degrees"
        );
        assert!(!t.contains([3.0, 0.0, 0.0]), "the short axis is now on +x");
    }

    /// Retail divides by `size` rather than comparing against half of it
    /// (RidManager.cpp RidManager::Add), so a mirrored extent keeps the same volume
    /// instead of collapsing the box to nothing.
    #[test]
    fn a_mirrored_extent_still_bounds_the_box() {
        let t = trigger(0x1C6, [0.0; 3], 0.0, [-4.0, 8.0, 2.0]);
        assert!(t.contains([1.9, 0.0, 0.0]));
        assert!(t.contains([-1.9, 0.0, 0.0]));
        assert!(!t.contains([2.1, 0.0, 0.0]));
    }

    #[test]
    fn a_zero_extent_box_holds_nothing() {
        let t = trigger(0x1C6, [0.0; 3], 0.0, [0.0, 8.0, 2.0]);
        assert!(!t.contains([0.0; 3]));
        assert!(!t.contains([1.0, 0.0, 0.0]));
    }

    const SHOP: u32 = 0x1C6;
    /// The furthest a Port Jeuno doorway rect was measured to stand off the shell
    /// it opens; [`SUB_AREA_SHELL_CLEAR_MARGIN`] has to reach past it.
    const MEASURED_MAX_DOORWAY_STANDOFF: f32 = 5.6;

    /// A doorway rect just outside the shell, the way the shipped ones sit: the
    /// building spans x 0..20, the rect straddles x = -1.
    fn shop_house() -> (Vec<ZoneInteraction>, Vec<SubAreaShell>) {
        (
            vec![trigger(SHOP, [-1.0, 0.0, 10.0], 0.0, [4.0, 8.0, 6.0])],
            vec![SubAreaShell {
                id: SHOP,
                min: [0.0, -4.0, 0.0],
                max: [20.0, 12.0, 20.0],
            }],
        )
    }

    #[test]
    fn the_latch_sets_on_entering_a_trigger_and_holds_past_it() {
        let (triggers, shells) = shop_house();
        let mut latch = SubAreaLatch::new(&triggers, shells);

        assert_eq!(latch.update([-10.0, 0.0, 10.0]), None, "approaching");
        assert_eq!(
            latch.update([-1.0, 0.0, 10.0]),
            Some(SHOP),
            "in the doorway"
        );
        assert_eq!(
            latch.update([10.0, 0.0, 10.0]),
            Some(SHOP),
            "deep inside, past every rect"
        );
    }

    #[test]
    fn the_latch_clears_only_once_clear_of_the_inflated_shell() {
        let (triggers, shells) = shop_house();
        let mut latch = SubAreaLatch::new(&triggers, shells);
        latch.update([-1.0, 0.0, 10.0]);
        latch.update([10.0, 0.0, 10.0]);

        assert_eq!(
            latch.update([-1.0, 0.0, 10.0]),
            Some(SHOP),
            "back in the door"
        );
        assert_eq!(
            latch.update([-MEASURED_MAX_DOORWAY_STANDOFF, 0.0, 10.0]),
            Some(SHOP),
            "outside every rect but no further out than a real doorway stands"
        );
        assert_eq!(
            latch.update([-(SUB_AREA_SHELL_CLEAR_MARGIN + 1.0), 0.0, 10.0]),
            None,
            "past the margin"
        );
    }

    #[test]
    fn leaving_by_a_different_door_of_the_same_building_still_clears() {
        let west = trigger(SHOP, [-1.0, 0.0, 10.0], 0.0, [4.0, 8.0, 6.0]);
        let east = trigger(SHOP, [21.0, 0.0, 10.0], 0.0, [4.0, 8.0, 6.0]);
        let shells = vec![SubAreaShell {
            id: SHOP,
            min: [0.0, -4.0, 0.0],
            max: [20.0, 12.0, 20.0],
        }];
        let mut latch = SubAreaLatch::new(&[west, east], shells);

        latch.update([-10.0, 0.0, 10.0]);
        assert_eq!(latch.update([-1.0, 0.0, 10.0]), Some(SHOP));
        assert_eq!(latch.update([10.0, 0.0, 10.0]), Some(SHOP));
        assert_eq!(
            latch.update([21.0, 0.0, 10.0]),
            Some(SHOP),
            "the far door re-arms the same id"
        );
        assert_eq!(
            latch.update([20.0 + SUB_AREA_SHELL_CLEAR_MARGIN + 1.0, 0.0, 10.0]),
            None
        );
    }

    #[test]
    fn a_zero_param_rect_clears_the_latch_where_it_stands() {
        let (mut triggers, shells) = shop_house();
        triggers.push(trigger(0, [10.0, 0.0, 10.0], 0.0, [2.0, 8.0, 2.0]));
        let mut latch = SubAreaLatch::new(&triggers, shells);

        latch.update([-1.0, 0.0, 10.0]);
        assert_eq!(latch.active(), Some(SHOP));
        assert_eq!(
            latch.update([10.0, 0.0, 10.0]),
            None,
            "the leave rect clears while still inside the shell"
        );
    }

    #[test]
    fn entering_a_second_building_swaps_the_active_sub_area() {
        let a = trigger(SHOP, [-1.0, 0.0, 10.0], 0.0, [4.0, 8.0, 6.0]);
        let b = trigger(SHOP + 1, [99.0, 0.0, 10.0], 0.0, [4.0, 8.0, 6.0]);
        let mut latch = SubAreaLatch::new(&[a, b], Vec::new());
        latch.update([-1.0, 0.0, 10.0]);
        assert_eq!(latch.active(), Some(SHOP));
        assert_eq!(latch.update([99.0, 0.0, 10.0]), Some(SHOP + 1));
    }

    /// Without a shell the geometric clear has nothing to test against, so the
    /// fail-safe keeps the interior; only [`SubAreaLatch::reset`] or a leave rect
    /// drops it.
    #[test]
    fn a_sub_area_with_no_shell_is_never_cleared_geometrically() {
        let (triggers, _) = shop_house();
        let mut latch = SubAreaLatch::new(&triggers, Vec::new());
        latch.update([-1.0, 0.0, 10.0]);
        assert_eq!(latch.update([9999.0, 0.0, 9999.0]), Some(SHOP));
        latch.reset();
        assert_eq!(latch.active(), None);
    }

    #[test]
    fn the_latch_ignores_rects_that_are_not_sub_area_triggers() {
        let mut region = trigger(SHOP, [0.0; 3], 0.0, [100.0; 3]);
        region.rect_class = 555;
        let mut zone_line = trigger(SHOP + 1, [0.0; 3], 0.0, [100.0; 3]);
        zone_line.source_id = DatId(*b"zmr0");

        let mut latch = SubAreaLatch::new(&[region, zone_line], Vec::new());
        assert!(latch.triggers().is_empty());
        assert_eq!(latch.update([0.0; 3]), None);
    }

    /// Gated on a retail install (self-skips without one). Lower Jeuno is the
    /// worked example in research/cexi-docs/zone/subareas.md "Worked example — Lower Jeuno (`ROM/1/41`, zone 245)"; this pins
    /// our resolution against the shipped DATs rather than against that doc.
    #[test]
    fn lower_jeuno_sub_areas_resolve_to_real_interior_dats() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };

        const LOWER_JEUNO: u16 = 245;
        let file_id = crate::zone_dat::zone_id_to_mzb_file_id(LOWER_JEUNO).unwrap();
        let loc = root.resolve(file_id).unwrap();
        let bytes = std::fs::read(loc.path_under(&root)).unwrap();

        let subs = from_dat(&bytes).unwrap();
        assert_eq!(
            subs.iter().map(|s| s.id).collect::<Vec<_>>(),
            (0x1C6..=0x1D2).collect::<Vec<_>>(),
            "Lower Jeuno declares 13 consecutive sub-areas"
        );
        assert_eq!(subs[0].file_id, 0x22A);
        assert_eq!(subs[12].file_id, 0x236);

        for s in &subs {
            let loc = root
                .resolve(s.file_id)
                .unwrap_or_else(|e| panic!("sub-area {:#x} -> file {}: {e}", s.id, s.file_id));
            let interior = std::fs::read(loc.path_under(&root)).unwrap();
            let chunks: Vec<_> = crate::chunk::walk(&interior).flatten().collect();
            let mzb = chunks
                .iter()
                .find(|c| {
                    crate::kind::ChunkKind::from_u8(c.kind) == Some(crate::kind::ChunkKind::Mzb)
                })
                .unwrap_or_else(|| panic!("sub-area {:#x} interior has no MZB chunk", s.id));
            let plain = crate::mzb::decrypt(mzb.data).unwrap();
            let header = crate::mzb::MzbHeader::parse(&plain).unwrap();
            let placements = crate::mzb::parse_mmb_placements(&plain, &header).unwrap();
            assert!(
                !placements.is_empty(),
                "sub-area {:#x} interior is empty",
                s.id
            );
        }

        // Every placement link the zone carries is declared by an m-rect, so no
        // Lower Jeuno shell is stranded visible.
        let chunks: Vec<_> = crate::chunk::walk(&bytes).flatten().collect();
        let mzb = chunks
            .iter()
            .find(|c| crate::kind::ChunkKind::from_u8(c.kind) == Some(crate::kind::ChunkKind::Mzb))
            .unwrap();
        let plain = crate::mzb::decrypt(mzb.data).unwrap();
        let header = crate::mzb::MzbHeader::parse(&plain).unwrap();
        let placements = crate::mzb::parse_mmb_placements(&plain, &header).unwrap();
        assert_eq!(undeclared_placeholder_links(&subs, &placements), Vec::new());

        let linked: Vec<u32> = {
            let mut v: Vec<u32> = placements
                .iter()
                .map(|p| p.sub_area_link)
                .filter(|l| *l != 0)
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        assert_eq!(
            linked,
            subs.iter().map(|s| s.id).collect::<Vec<_>>(),
            "every declared sub-area has a placeholder shell to hide"
        );
    }

    /// The high-offset branch, which only the retail DATs can prove.
    #[test]
    fn high_offset_sub_areas_resolve_to_real_interior_dats() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };

        const HIGH_OFFSET_ZONE: u16 = 289;
        let file_id = crate::zone_dat::zone_id_to_mzb_file_id(HIGH_OFFSET_ZONE).unwrap();
        let loc = root.resolve(file_id).unwrap();
        let bytes = std::fs::read(loc.path_under(&root)).unwrap();

        let subs = from_dat(&bytes).unwrap();
        assert_eq!(
            subs.iter().map(|s| s.id).collect::<Vec<_>>(),
            (0x271..=0x280).collect::<Vec<_>>()
        );
        for s in &subs {
            assert!(
                s.id >= SUB_AREA_ID_HIGH_MIN,
                "zone {HIGH_OFFSET_ZONE} is the high-offset case"
            );
            assert!(
                root.resolve(s.id + SUB_AREA_FILE_ID_OFFSET).is_err(),
                "sub-area {:#x} must not resolve under the low offset",
                s.id
            );
            let loc = root.resolve(s.file_id).unwrap();
            let interior = std::fs::read(loc.path_under(&root)).unwrap();
            assert!(crate::chunk::walk(&interior)
                .flatten()
                .any(|c| crate::kind::ChunkKind::from_u8(c.kind)
                    == Some(crate::kind::ChunkKind::Mzb)));
        }
    }
}
