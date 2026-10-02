use super::*;

#[derive(Debug, Clone, Copy)]
pub struct ItemMax {
    pub capacities: [u16; Self::CONTAINER_COUNT],
}

impl ItemMax {
    /// One capacity per LSB CONTAINER_ID (LOC_INVENTORY..=LOC_RECYCLEBIN),
    /// vendor/server/src/map/item_container.h CONTAINER_ID LOC_INVENTORY.
    pub const CONTAINER_COUNT: usize = 18;
    pub(crate) const SIZE: usize = 96;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut capacities = [0u16; Self::CONTAINER_COUNT];
        // Fall back to the legacy u8 array only when the whole wide array is
        // absent (pre-widening servers). A per-slot fallback would erase LSB's
        // "container disabled" sentinel — ItemNum2 = 0 while the legacy byte
        // stays sized, e.g. a lapsed Mog Locker lease
        // (vendor/server/src/map/packets/s2c/0x01c_item_max.cpp GP_SERV_COMMAND_ITEM_MAX::GP_SERV_COMMAND_ITEM_MAX).
        let wide_at = |i: usize| {
            let off = 18 + 14 + i * 2;
            u16::from_le_bytes(body[off..off + 2].try_into().unwrap())
        };
        let wide_present = (0..Self::CONTAINER_COUNT).any(|i| wide_at(i) != 0);
        for (i, cap) in capacities.iter_mut().enumerate() {
            let raw = if wide_present {
                wide_at(i)
            } else {
                body[i] as u16
            };
            *cap = raw.saturating_sub(1);
        }
        Ok(Self { capacities })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSameState {
    StillLoading,
    AllLoaded,
}

#[derive(Debug, Clone, Copy)]
pub struct ItemSame {
    pub state: ItemSameState,
    pub flags: u32,
}

impl ItemSame {
    pub(crate) const SIZE: usize = 8;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let state = match body[0] {
            0 => ItemSameState::StillLoading,

            _ => ItemSameState::AllLoaded,
        };
        let flags = u32::from_le_bytes(body[4..8].try_into().unwrap());
        Ok(Self { state, flags })
    }
}

/// Slot lock flags the server stamps on an item.
/// vendor/server/src/map/enums/item_lockflg.h ItemLockFlg.
pub mod lock_flg {
    /// The slot cannot be picked: LSB raises it for the span of an item use, a synth or a trade
    /// offer and clears it when that resolves (vendor/server/src/map/ai/states/item_state.cpp
    /// CItemState::init and CItemState::Cleanup). For a use it is the only signal that lands
    /// before the ITEM_ATTR carrying the new recast, which LSB only sends at the span's end.
    pub const NO_SELECT: u8 = 0x0F;
}

#[derive(Debug, Clone, Copy)]
pub struct ItemNum {
    pub quantity: u32,

    pub category: u8,

    pub index: u8,

    pub lock_flg: u8,
}

impl ItemNum {
    pub(crate) const SIZE: usize = 8;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            quantity: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            category: body[4],
            index: body[5],
            lock_flg: body[6],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemList {
    pub quantity: u32,

    pub item_no: u16,
    pub category: u8,
    pub index: u8,
    pub lock_flg: u8,
}

impl ItemList {
    pub(crate) const SIZE: usize = 12;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            quantity: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            item_no: u16::from_le_bytes(body[4..6].try_into().unwrap()),
            category: body[6],
            index: body[7],
            lock_flg: body[8],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemAttr {
    pub quantity: u32,
    pub price: u32,
    pub item_no: u16,
    pub category: u8,
    pub index: u8,
    pub lock_flg: u8,
    pub extdata: [u8; 24],
}

/// Charged-item state from `ItemTimerInfo` extdata.
///
/// `vendor/server/src/map/packets/s2c/0x020_item_attr.cpp
/// GP_SERV_COMMAND_ITEM_ATTR::GP_SERV_COMMAND_ITEM_ATTR` leaves both timestamps
/// stale on the ready path, so `ready` is the authoritative readiness signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChargeInfo {
    pub charges: u8,
    pub next_use_vana_ts: u32,
    pub use_delay_end_vana_ts: u32,
    pub ready: bool,
}

/// Item extdata byte layout for charged items.
/// vendor/server/src/map/items/exdata/timer_info.h ItemTimerInfo and
/// vendor/server/src/map/packets/s2c/0x020_item_attr.cpp GP_SERV_COMMAND_ITEM_ATTR::GP_SERV_COMMAND_ITEM_ATTR.
mod extdata {
    use core::ops::Range;
    pub(crate) const HEADER_CHARGED: u8 = 0x01;
    pub(crate) const OFF_HEADER: usize = 0;
    pub(crate) const OFF_CHARGES: usize = 1;
    pub(crate) const OFF_FLAGS_HI: usize = 3;
    pub(crate) const NEXT_USE: Range<usize> = 4..8;
    pub(crate) const USE_DELAY_END: Range<usize> = 8..12;
    pub(crate) const FLAG_READY: u8 = 0x40;
}

impl ItemAttr {
    pub(crate) const SIZE: usize = 37;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut extdata = [0u8; 24];
        extdata.copy_from_slice(&body[13..37]);
        Ok(Self {
            quantity: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            price: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            item_no: u16::from_le_bytes(body[8..10].try_into().unwrap()),
            category: body[10],
            index: body[11],
            lock_flg: body[12],
            extdata,
        })
    }

    /// Charge/recast info for a charged (usable/enchanted) item, or `None` when
    /// the extdata header marks it as non-charged.
    pub fn charge_info(&self) -> Option<ChargeInfo> {
        if self.extdata[extdata::OFF_HEADER] != extdata::HEADER_CHARGED {
            return None;
        }
        Some(ChargeInfo {
            charges: self.extdata[extdata::OFF_CHARGES],
            next_use_vana_ts: u32::from_le_bytes(
                self.extdata[extdata::NEXT_USE].try_into().unwrap(),
            ),
            use_delay_end_vana_ts: u32::from_le_bytes(
                self.extdata[extdata::USE_DELAY_END].try_into().unwrap(),
            ),
            ready: self.extdata[extdata::OFF_FLAGS_HI] & extdata::FLAG_READY != 0,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EquipList {
    pub container_index: u8,

    pub equip_slot: u8,

    pub container: u8,
}

impl EquipList {
    pub(crate) const SIZE: usize = 4;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            container_index: body[0],
            equip_slot: body[1],
            container: body[2],
        })
    }
}

#[cfg(test)]
mod item_tests {
    use super::*;

    #[test]
    fn item_max_decodes_legacy_and_wide_capacity() {
        let mut buf = vec![0u8; ItemMax::SIZE];

        buf[0] = 81;

        buf[1] = 81;
        let wide_off = 18 + 14 + 2;
        buf[wide_off..wide_off + 2].copy_from_slice(&201u16.to_le_bytes());

        let wide_off = 18 + 14 + 10 * 2;
        buf[wide_off..wide_off + 2].copy_from_slice(&81u16.to_le_bytes());

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(
            m.capacities[1], 200,
            "Mog Safe: wide takes precedence, +1 inverted"
        );
        assert_eq!(m.capacities[10], 80, "Wardrobe2: wide-only, +1 inverted");
        assert_eq!(
            m.capacities[17], 0,
            "Recycle Bin: zeroed (disabled sentinel)"
        );
    }

    /// LSB's only ItemNum2 = 0 emitter is a DISABLED container (a lapsed Mog
    /// Locker lease keeps its legacy byte sized), so once any wide value is
    /// present a zero must stay zero rather than fall back per-slot
    /// (vendor/server/src/map/packets/s2c/0x01c_item_max.cpp GP_SERV_COMMAND_ITEM_MAX::GP_SERV_COMMAND_ITEM_MAX).
    #[test]
    fn item_max_wide_zero_is_the_disable_sentinel_not_a_fallback() {
        let mut buf = vec![0u8; ItemMax::SIZE];
        buf[0] = 31;
        buf[4] = 31; // lapsed locker: legacy still sized...
        let wide_off = 18 + 14;
        buf[wide_off..wide_off + 2].copy_from_slice(&31u16.to_le_bytes());
        // ...but ItemNum2[LOC_MOGLOCKER] stays 0.

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(m.capacities[0], 30);
        assert_eq!(m.capacities[4], 0, "wide 0 = disabled, no legacy fallback");
    }

    #[test]
    fn item_max_falls_back_to_legacy_only_when_wide_is_absent() {
        let mut buf = vec![0u8; ItemMax::SIZE];

        buf[0] = 81;
        buf[4] = 21;

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(m.capacities[0], 80, "pre-widening server: legacy decoded");
        assert_eq!(m.capacities[4], 20, "moglocker: legacy decoded with -1");
        assert_eq!(
            m.capacities[1], 0,
            "fully-disabled stays at 0, no underflow"
        );
    }

    #[test]
    fn item_max_truncated_errors() {
        let buf = vec![0u8; ItemMax::SIZE - 1];
        assert!(matches!(
            ItemMax::decode(&buf),
            Err(DecodeError::Truncated(96, _))
        ));
    }

    #[test]
    fn item_same_decodes_state_and_flags() {
        let mut buf = vec![0u8; ItemSame::SIZE];
        buf[0] = 0;
        buf[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        let s = ItemSame::decode(&buf).unwrap();
        assert_eq!(s.state, ItemSameState::StillLoading);
        assert_eq!(s.flags, 0xCAFE);

        buf[0] = 1;
        let s = ItemSame::decode(&buf).unwrap();
        assert_eq!(s.state, ItemSameState::AllLoaded);
    }

    #[test]
    fn item_num_decodes() {
        let mut buf = vec![0u8; ItemNum::SIZE];
        buf[0..4].copy_from_slice(&12345u32.to_le_bytes());
        buf[4] = 0;
        buf[5] = 7;
        buf[6] = 1;
        let n = ItemNum::decode(&buf).unwrap();
        assert_eq!(n.quantity, 12345);
        assert_eq!(n.category, 0);
        assert_eq!(n.index, 7);
        assert_eq!(n.lock_flg, 1);
    }

    /// The lock byte is the only field an item use rewrites, and it sits right
    /// after the slot index - reading either a byte off swaps two small numbers
    /// with no other symptom. (The flag's value is pinned by the ItemLockFlg
    /// citation, not by this test.)
    #[test]
    fn item_list_decodes() {
        let mut buf = vec![0u8; ItemList::SIZE];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[4..6].copy_from_slice(&4112u16.to_le_bytes());
        buf[6] = 5;
        buf[7] = 12;
        buf[8] = 0;
        let l = ItemList::decode(&buf).unwrap();
        assert_eq!(l.quantity, 1);
        assert_eq!(l.item_no, 4112);
        assert_eq!(l.category, 5);
        assert_eq!(l.index, 12);
        assert_eq!(l.lock_flg, 0);

        buf[8] = lock_flg::NO_SELECT;
        let locked = ItemList::decode(&buf).unwrap();
        assert_eq!(locked.lock_flg, lock_flg::NO_SELECT);
        assert_eq!(locked.index, 12, "the index is its own byte");
    }

    #[test]
    fn item_attr_decodes_with_extdata() {
        let mut buf = vec![0u8; ItemAttr::SIZE];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[4..8].copy_from_slice(&500_000u32.to_le_bytes());
        buf[8..10].copy_from_slice(&8000u16.to_le_bytes());
        buf[10] = 0;
        buf[11] = 3;
        buf[12] = 0;
        for (i, b) in buf[13..37].iter_mut().enumerate() {
            *b = i as u8;
        }
        let a = ItemAttr::decode(&buf).unwrap();
        assert_eq!(a.quantity, 1);
        assert_eq!(a.price, 500_000);
        assert_eq!(a.item_no, 8000);
        assert_eq!(a.category, 0);
        assert_eq!(a.index, 3);
        assert_eq!(a.extdata[0], 0);
        assert_eq!(a.extdata[23], 23);
    }

    #[test]
    fn item_attr_truncated_errors() {
        let buf = vec![0u8; ItemAttr::SIZE - 1];
        assert!(matches!(
            ItemAttr::decode(&buf),
            Err(DecodeError::Truncated(37, _))
        ));
    }

    fn item_attr_with_extdata(ext: [u8; 24]) -> ItemAttr {
        let mut buf = vec![0u8; ItemAttr::SIZE];
        buf[13..37].copy_from_slice(&ext);
        ItemAttr::decode(&buf).unwrap()
    }

    #[test]
    fn charge_info_none_for_non_charged_header() {
        let mut ext = [0u8; 24];
        ext[0] = 0x00;
        assert_eq!(item_attr_with_extdata(ext).charge_info(), None);
    }

    #[test]
    fn charge_info_reads_charges_timestamps_and_ready() {
        let mut ext = [0u8; 24];
        ext[0] = 0x01;
        ext[1] = 2;
        ext[3] = 0x90;
        ext[4..8].copy_from_slice(&123_456u32.to_le_bytes());
        ext[8..12].copy_from_slice(&123_486u32.to_le_bytes());
        let ci = item_attr_with_extdata(ext).charge_info().unwrap();
        assert_eq!(ci.charges, 2);
        assert_eq!(ci.next_use_vana_ts, 123_456);
        assert_eq!(ci.use_delay_end_vana_ts, 123_486);
        assert!(!ci.ready);
    }

    #[test]
    fn charge_info_ready_bit_set_and_zero_timestamp() {
        let mut ext = [0u8; 24];
        ext[0] = 0x01;
        ext[1] = 1;
        ext[3] = 0xC0;
        let ci = item_attr_with_extdata(ext).charge_info().unwrap();
        assert_eq!(ci.charges, 1);
        assert_eq!(ci.next_use_vana_ts, 0);
        assert_eq!(ci.use_delay_end_vana_ts, 0);
        assert!(ci.ready);
    }
}

#[cfg(test)]
mod equip_list_tests {
    use super::*;

    #[test]
    fn equip_list_decodes_field_order() {
        let buf = [0x05u8, 0x04, 0x08, 0x00];
        let e = EquipList::decode(&buf).expect("decode");
        assert_eq!(e.container_index, 5);
        assert_eq!(e.equip_slot, 4);
        assert_eq!(e.container, 8);
    }

    #[test]
    fn equip_list_truncated_returns_err() {
        let buf = [0u8; EquipList::SIZE - 1];
        assert!(matches!(
            EquipList::decode(&buf),
            Err(DecodeError::Truncated(EquipList::SIZE, n)) if n == EquipList::SIZE - 1
        ));
    }
}
