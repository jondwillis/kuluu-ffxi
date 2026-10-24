//! The Mog House furniture ingest law.
//!
//! Placed furniture is *item state*, not myroom state: the retail client rebuilds its furniture
//! object table from the s2c 0x020 `GP_SERV_COMMAND_ITEM_ATTR` inventory records and ignores layout
//! echoes — the handler registered for s2c 0x0FA is a two-instruction `mov al, 1; ret` stub
//! (FFXiMain.dll retail-2026-09 `.text 0xF0C70`). Every field an object needs therefore arrives in
//! the item record's 24-byte Attr ([`crate::state::ItemSlot::extdata`], kept verbatim), so this
//! module is pure: given the shelf records it produces exactly what retail's table holds.
//!
//! The rebuild entry point (FFXiMain.dll retail-2026-09 `.text 0x1D2AF0`) walks two containers — it
//! pushes container 1 and then container 9 before each call to the per-container scan at
//! `.text 0x1D2B10`, which is LSB's own Mog House pair
//! (vendor/server/src/map/entities/char_entity.cpp `CCharEntity::UpdateMoghancement` iterates
//! `{ LOC_MOGSAFE, LOC_MOGSAFE2 }`, slots `1..=GetSize()`). Slots outside 1..=80 are rejected by the
//! scan itself (`cmp edi, 0x51; jge skip`, `.text 0x1D2B98`).
//!
//! A record becomes a furniture object when all three hold:
//! 1. its DAT kind is 10, 12 or 14 — the byte the item lookup returns and the scan compares at
//!    `.text 0x1D2C3C`. The same byte in our reader is
//!    [`ffxi_dat::item_dat::ItemStatic::item_type`]: those three values cover exactly the
//!    beds/tables/chests (479 records), flowerpots (10) and mannequins (8) of a retail-2026-09 item
//!    DAT, so no kind table is duplicated here;
//! 2. the Attr's installed bit is set — `extdata[1] & 0x40` (`.text 0x1D2C4B`; LSB calls the same
//!    bit `Exdata::Furniture::Installed`, vendor/server/src/map/items/exdata/furniture.h);
//! 3. its two-bit layer, `extdata[1] & 3`, equals the character's current Mog House floor
//!    (`.text 0x1D2C56`). An installed item standing on another floor is taken out of the object
//!    table through the removal path `.text 0x1D4BA0`.
//!
//! Accepted records land in a flat array at myroom `+0x4C`, indexed
//! `slot + (container == 9 ? 81 : 0)` — visible in the create path's arithmetic
//! (`.text 0x1D3A01`). Each object's pose is copied out of Attr without conversion (`.text 0x1CCFD1`):
//! layer `extdata[1] & 3`, then signed bytes `extdata[6]`, `extdata[7]`, `extdata[8]`, rotation
//! `extdata[9]`. LSB declares those four bytes `X`, `Z`, `Y`, `Rotation` — Z before Y, *not* X/Y/Z
//! (vendor/server/src/map/items/exdata/furniture.h) — but its own furniture accessors say what the
//! three pose bytes mean: `getCol()` reads `X`, `getLevel()` reads `Z`, `getRow()` reads `Y`
//! (vendor/server/src/map/items/item_furnishing.cpp), and the layout request it answers with feeds
//! those same three positions in that order (`setCol(this->x)`, `setLevel(this->y)`, `setRow(this->z)`
//! in vendor/server/src/map/packets/c2s/0x0fa_myroom_layout.cpp). This module therefore names them
//! `col`, `level` and `row` so nothing downstream has to re-derive the letter mapping.

use serde::{Deserialize, Serialize};

/// Mog House storage shelf. FFXiMain.dll retail-2026-09 pushes 1 to the per-container furniture scan
/// at `.text 0x1D2AFC`; LSB calls it `LOC_MOGSAFE` (vendor/server/src/map/item_container.h).
pub const CONTAINER_MOGSAFE: u8 = ffxi_proto::map::container::LOC_MOGSAFE;

/// The second Mog House shelf, pushed at `.text 0x1D2B03`; LSB's `LOC_MOGSAFE2`. It occupies the
/// back half of the object array.
pub const CONTAINER_MOGSAFE2: u8 = ffxi_proto::map::container::LOC_MOGSAFE2;

/// First slot the scan accepts (a slot byte of 0 fails at FFXiMain.dll retail-2026-09
/// `.text 0x1D2B90`, matching LSB's loop start).
pub const FIRST_FURNITURE_SLOT: u8 = ffxi_dat::furniture::FIRST_SHELF_SLOT;

/// Last slot the scan accepts: it rejects a slot byte `>= 0x51` at FFXiMain.dll retail-2026-09
/// `.text 0x1D2B98`.
pub const LAST_FURNITURE_SLOT: u8 = ffxi_dat::furniture::LAST_SHELF_SLOT;

/// Object-array seats per container, from the `* 81` in FFXiMain.dll retail-2026-09 `.text 0x1D3A01`.
pub const FURNITURE_SLOTS_PER_CONTAINER: usize = ffxi_dat::furniture::SEATS_PER_SHELF;

/// `extdata[1] & INSTALLED_BIT`: the item is placed in the room rather than stored on the shelf
/// (FFXiMain.dll retail-2026-09 `.text 0x1D2C4B`; LSB's `Exdata::Furniture::Installed`).
pub const FURNITURE_INSTALLED_BIT: u8 = ffxi_dat::furniture::ATTR_INSTALLED_BIT;

/// `extdata[1] & LAYER_MASK`: which floor of the Mog House the object stands on. Both sides agree on
/// those bits — LSB sets them from its own layout request's floor flag and calls bit 0
/// `On2ndFloor` (vendor/server/src/map/packets/c2s/0x0fa_myroom_layout.cpp
/// `C2SPOrPacketImpl... MyroomFloorFlg`), FFXiMain.dll retail-2026-09 masks the same two bits at
/// `.text 0x1D2C59`.
pub const FURNITURE_LAYER_MASK: u8 = ffxi_dat::furniture::ATTR_LAYER_MASK;

/// DAT kind accepted by the scan's first compare (FFXiMain.dll retail-2026-09 `.text 0x1D2C3C`).
pub const FURNITURE_KIND_PLAIN: u8 = ffxi_dat::furniture::KIND_FURNISHING;
/// Flowerpots, reached by subtracting twice at `.text 0x1D2C41`.
pub const FURNITURE_KIND_FLOWERPOT: u8 = ffxi_dat::furniture::KIND_FLOWERPOT;
/// Mannequins (`.text 0x1D2C46`).
pub const FURNITURE_KIND_MANNEQUIN: u8 = ffxi_dat::furniture::KIND_MANNEQUIN;

/// The two containers the retail rebuild walks.
pub fn is_mog_house_shelf(container: u8) -> bool {
    container == CONTAINER_MOGSAFE || container == CONTAINER_MOGSAFE2
}

/// Whether a slot byte can name a seat at all when something is *selected* (as opposed to ingested):
/// the mode switch only rejects `slot >= 0x51` here (FFXiMain.dll retail-2026-09 `.text 0x1D3656`, and
/// again for select-one-piece mode at `.text 0x1D374A`), so slot 0 passes this test even though the
/// record-driven rebuild never places anything in it.
pub fn is_selectable_slot(slot: u8) -> bool {
    slot <= LAST_FURNITURE_SLOT
}

/// The DAT kinds that count as furniture (the three-way compare at `.text 0x1D2C3C`).
pub fn is_furniture_kind(kind: u8) -> bool {
    ffxi_dat::furniture::is_furniture_kind(kind)
}

/// The floor the layer test compares against. Retail keeps it in a game-state word (`+0x40D82`,
/// getter `.text 0xF1080`) written only while parsing s2c 0x00A: bits 1-2 of `MyroomSubMapNumber`
/// (`.text 0xFAEFB` reads the byte, `.text 0xFAF01` shifts it). LSB sends that byte as 2 for a
/// second-floor Mog House interior (vendor/server/src/map/packets/s2c/0x00a_login.cpp), so an
/// upstairs character sees floor 1 and their downstairs furniture leaves the table.
pub fn current_furniture_floor(sub_map_number: u8) -> u8 {
    ffxi_dat::furniture::floor_of_sub_map(sub_map_number)
}

/// Where an item id's DAT kind comes from, so the law can be applied without copying item records
/// anywhere and tested without an install.
pub trait ItemKindSource: Send + Sync {
    /// The byte retail compares against 10/12/14, i.e.
    /// [`ffxi_dat::item_dat::ItemStatic::item_type`]. `None` when the install has no record.
    fn item_kind(&self, item_no: u16) -> Option<u8>;
}

impl ItemKindSource for ffxi_dat::item_dat::ItemTable {
    fn item_kind(&self, item_no: u16) -> Option<u8> {
        self.lookup(item_no).map(|record| record.item_type)
    }
}

/// A kind source from a plain table: the fixture for these tests and for any caller that already
/// holds the kinds it wants to answer.
#[derive(Debug, Default, Clone)]
pub struct StaticItemKinds(pub Vec<(u16, u8)>);

impl ItemKindSource for StaticItemKinds {
    fn item_kind(&self, item_no: u16) -> Option<u8> {
        self.0
            .iter()
            .find(|(id, _)| *id == item_no)
            .map(|(_, k)| *k)
    }
}

/// One placed furniture object: where it came from plus the layer and pose retail copies out of Attr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedFurniture {
    /// Object-array seat: `slot + (container == 9 ? 81 : 0)` (FFXiMain.dll retail-2026-09
    /// `.text 0x1D3A01`), so the same slot on either shelf has its own stable index.
    pub index: usize,
    pub container: u8,
    pub slot: u8,
    pub item_no: u16,
    /// `extdata[1] & 3`, already matched against [`current_furniture_floor`].
    pub layer: u8,
    /// `extdata[6]` as the signed byte retail stores it — LSB `Exdata::Furniture::X`, which its
    /// furniture accessors call the column (`getCol()`/`setCol()`).
    pub col: i8,
    /// `extdata[7]` (LSB's `Z`, and what `getLevel()`/`setLevel()` expose): how far above the floor
    /// the object sits. LSB's validator keeps it in 0..=25.
    pub level: i8,
    /// `extdata[8]` (LSB's `Y`, from `getRow()`/`setRow()`).
    pub row: i8,
    /// `extdata[9]`.
    pub rotation: i8,
}

/// Why a record is not a placed furniture object on the current floor. Every variant except
/// [`SkipReason::Placed`] mirrors a test the retail scan makes before touching the object array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The record reached the object table.
    Placed,
    /// Slot 0 or past the shelf: rejected at FFXiMain.dll retail-2026-09 `.text 0x1D2B8E`.
    SlotOutOfRange,
    /// No DAT record for the id — fail closed instead of guessing a kind.
    KindUnknown,
    /// DAT kind is none of 10/12/14 (`.text 0x1D2C3C`).
    NotFurniture,
    /// Installed bit clear (`.text 0x1D2C4B`): stored on the shelf, not placed in the room.
    NotInstalled,
    /// `extdata[1] & 3` named a different floor than the character's (`.text 0x1D2C56`).
    OtherFloor(u8),
}

/// Run the whole predicate stack over one inventory record and say why it passed or failed. The
/// caller supplies the DAT kind, which is the only thing not in the record itself; `.text 0x1D2C1C`
/// does the same via the client's own item lookup.
pub fn furniture_object(
    container: u8,
    slot: u8,
    item_no: u16,
    extdata: [u8; 24],
    current_floor: u8,
    kind_of: &mut dyn FnMut(u16) -> Option<u8>,
) -> (Option<PlacedFurniture>, SkipReason) {
    if !(FIRST_FURNITURE_SLOT..=LAST_FURNITURE_SLOT).contains(&slot) {
        return (None, SkipReason::SlotOutOfRange);
    }
    let Some(kind) = kind_of(item_no) else {
        return (None, SkipReason::KindUnknown);
    };
    if !is_furniture_kind(kind) {
        return (None, SkipReason::NotFurniture);
    }
    if extdata[1] & FURNITURE_INSTALLED_BIT == 0 {
        return (None, SkipReason::NotInstalled);
    }
    let layer = extdata[1] & FURNITURE_LAYER_MASK;
    if layer != current_floor {
        return (None, SkipReason::OtherFloor(layer));
    }
    (
        Some(describe_object(container, slot, item_no, &extdata)),
        SkipReason::Placed,
    )
}

/// The object an accepted record produces: index arithmetic and the four pose bytes, exactly as
/// FFXiMain.dll retail-2026-09 `.text 0x1D3A01` / `.text 0x1CCFD1` do it. Callers reach this only for
/// records [`furniture_object`] accepted; it applies no filtering of its own.
pub fn describe_object(
    container: u8,
    slot: u8,
    item_no: u16,
    extdata: &[u8; 24],
) -> PlacedFurniture {
    let attr = ffxi_dat::furniture::FurnitureAttr::decode(extdata);
    PlacedFurniture {
        index: furniture_index(container, slot),
        container,
        slot,
        item_no,
        layer: attr.layer,
        col: attr.pose.col,
        level: attr.pose.level,
        row: attr.pose.row,
        rotation: attr.pose.rot,
    }
}

/// Object-array seat of a Mog House shelf slot.
pub fn furniture_index(container: u8, slot: u8) -> usize {
    ffxi_dat::furniture::seat_index(container, slot)
}

/// Rebuild the object table from shelf records, sorted by index so a rebuild is stable. `records`
/// yields `(container, slot, item_no, extdata)`; entries outside the two Mog House shelves are
/// ignored (the retail scan never looks at them), and so are slots whose Attr has not arrived yet —
/// only s2c 0x020 carries it.
pub fn rebuild_furniture_objects(
    records: impl IntoIterator<Item = (u8, u8, u16, Option<[u8; 24]>)>,
    current_floor: u8,
    kinds: &dyn ItemKindSource,
) -> Vec<PlacedFurniture> {
    let mut kind_of = |item_no: u16| kinds.item_kind(item_no);
    let mut out: Vec<PlacedFurniture> = records
        .into_iter()
        .filter(|(container, _, _, _)| is_mog_house_shelf(*container))
        .filter_map(|(container, slot, item_no, extdata)| {
            let extdata = extdata?;
            furniture_object(
                container,
                slot,
                item_no,
                extdata,
                current_floor,
                &mut kind_of,
            )
            .0
        })
        .collect();
    out.sort_by_key(|o| o.index);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> StaticItemKinds {
        StaticItemKinds(vec![
            (100, FURNITURE_KIND_PLAIN),
            (200, FURNITURE_KIND_FLOWERPOT),
            (300, FURNITURE_KIND_MANNEQUIN),
            (400, 5),
        ])
    }

    fn attr(layer: u8, installed: bool, col: i8, level: i8, row: i8, rot: i8) -> [u8; 24] {
        let mut e = [0u8; 24];
        e[1] = (layer & FURNITURE_LAYER_MASK)
            | if installed {
                FURNITURE_INSTALLED_BIT
            } else {
                0
            };
        e[6] = col as u8;
        e[7] = level as u8;
        e[8] = row as u8;
        e[9] = rot as u8;
        e
    }

    fn check(
        container: u8,
        slot: u8,
        item_no: u16,
        extdata: [u8; 24],
        floor: u8,
    ) -> (Option<PlacedFurniture>, SkipReason) {
        let kinds = kinds();
        furniture_object(container, slot, item_no, extdata, floor, &mut |id| {
            kinds.item_kind(id)
        })
    }

    #[test]
    fn all_three_dat_kinds_are_accepted() {
        for id in [100u16, 200, 300] {
            let (obj, why) = check(CONTAINER_MOGSAFE, 5, id, attr(0, true, 0, 0, 0, 0), 0);
            assert_eq!(why, SkipReason::Placed, "id {id}");
            assert_eq!(obj.map(|o| o.item_no), Some(id));
        }
    }

    #[test]
    fn a_non_furniture_kind_is_skipped() {
        let (_, why) = check(CONTAINER_MOGSAFE, 5, 400, attr(0, true, 0, 0, 0, 0), 0);
        assert_eq!(why, SkipReason::NotFurniture);
    }

    #[test]
    fn an_unknown_kind_fails_closed() {
        let (_, why) = check(CONTAINER_MOGSAFE, 5, 999, attr(0, true, 0, 0, 0, 0), 0);
        assert_eq!(why, SkipReason::KindUnknown);
    }

    #[test]
    fn an_uninstalled_item_stays_on_the_shelf() {
        let (_, why) = check(CONTAINER_MOGSAFE, 5, 100, attr(0, false, 0, 0, 0, 0), 0);
        assert_eq!(why, SkipReason::NotInstalled);
    }

    #[test]
    fn only_the_current_floor_is_placed() {
        let (obj, why) = check(CONTAINER_MOGSAFE, 5, 100, attr(1, true, 0, 0, 0, 0), 0);
        assert_eq!((obj, why), (None, SkipReason::OtherFloor(1)));
        let (_, why) = check(CONTAINER_MOGSAFE, 5, 100, attr(1, true, 0, 0, 0, 0), 1);
        assert_eq!(why, SkipReason::Placed);
    }

    #[test]
    fn slot_zero_and_slot_81_are_out_of_range() {
        let ext = attr(0, true, 0, 0, 0, 0);
        for slot in [0u8, 81, 255] {
            let (_, why) = check(CONTAINER_MOGSAFE, slot, 100, ext, 0);
            assert_eq!(why, SkipReason::SlotOutOfRange, "slot {slot}");
        }
        assert_eq!(u16::from(LAST_FURNITURE_SLOT) + 1, 81);
    }

    #[test]
    fn the_second_shelf_lands_in_the_back_half_of_the_object_array() {
        let ext = attr(0, true, 3, -4, 5, -6);
        let one = check(CONTAINER_MOGSAFE, 7, 100, ext, 0).0.unwrap();
        let two = check(CONTAINER_MOGSAFE2, 7, 100, ext, 0).0.unwrap();
        assert_eq!(one.index, 7);
        assert_eq!(two.index, FURNITURE_SLOTS_PER_CONTAINER + 7);

        // Pose: the signed bytes, in the DAT's own byte order = LSB's Col/Level/Row accessors.
        for o in [one, two] {
            assert_eq!((o.col, o.level, o.row, o.rotation), (3, -4, 5, -6));
            assert_eq!(o.layer, 0);
        }
    }

    #[test]
    fn the_floor_word_is_bits_one_and_two_of_the_login_sub_map_number() {
        assert_eq!(current_furniture_floor(0), 0);
        // The value LSB sends for a second-floor Mog House interior (0x00a_login.cpp).
        assert_eq!(current_furniture_floor(2), 1);
        assert_eq!(current_furniture_floor(4), 2);
        assert_eq!(current_furniture_floor(6), 3);
        // Bit 0 is not part of the floor.
        assert_eq!(current_furniture_floor(3), 1);
    }

    #[test]
    fn a_rebuild_walks_only_the_two_mog_house_shelves() {
        let mut records = vec![
            (
                CONTAINER_MOGSAFE,
                2u8,
                100u16,
                Some(attr(0, true, 0, 0, 0, 0)),
            ),
            (
                CONTAINER_MOGSAFE2,
                3u8,
                200u16,
                Some(attr(0, true, 0, 0, 0, 0)),
            ),
            // A bag: the retail scan never looks at it.
            (0, 4u8, 100u16, Some(attr(0, true, 0, 0, 0, 0))),
            // On a Mog House shelf but with no Attr yet — only s2c 0x020 carries one.
            (CONTAINER_MOGSAFE, 5u8, 100u16, None),
        ];
        let objects = rebuild_furniture_objects(records.iter().copied(), 0, &kinds());
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].index, 2);
        assert_eq!(objects[1].index, FURNITURE_SLOTS_PER_CONTAINER + 3);

        // A floor change keeps only that layer's objects.
        records.push((
            CONTAINER_MOGSAFE2,
            6u8,
            300u16,
            Some(attr(1, true, 0, 0, 0, 0)),
        ));
        let upstairs = rebuild_furniture_objects(records.iter().copied(), 1, &kinds());
        assert_eq!(upstairs.iter().map(|o| o.slot).collect::<Vec<_>>(), vec![6]);
    }
}
