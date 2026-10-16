//! Player-to-player trade window bodies: s2c 0x021/0x022/0x023/0x025.
//! Wire law: vendor/server/src/map/items/transactions/player_trade.cpp.

use crate::decode::DecodeError;
use crate::s2c_layout::item_trade_list as list_lsb;
use crate::s2c_layout::item_trade_mylist as mylist_lsb;
use crate::s2c_layout::item_trade_req as req_lsb;
use crate::s2c_layout::item_trade_res as res_lsb;

/// ITEMID::GIL (vendor/server/src/map/items.h) — the same 0xFFFF LSB's trade code
/// uses for both "this slot holds gil" and "empty slot" (the quantity separates:
/// player_trade.cpp canReceive sums `slot.itemId == 0xFFFF` as offered gil, and
/// 0x025_item_trade_mylist.cpp writes the sentinel with ItemNum left at 0 to clear).
pub const TRADE_GIL_ITEM_NO: u16 = 0xFFFF;

/// PlayerTradeTransaction::MaxSlots — trade slot indices run 0..8, slot 0 is gil.
pub const TRADE_SLOT_COUNT: usize = 9;

/// s2c 0x021 ITEM_TRADE_REQ: someone asked to trade with us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTradeReq {
    pub unique_no: u32,
    pub act_index: u16,
}

impl ItemTradeReq {
    const BODY_LEN: usize = req_lsb::SIZE;
    const UNIQUE_NO_OFFSET: usize = 0x0;
    const ACT_INDEX_OFFSET: usize = 0x4;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::BODY_LEN {
            return Err(DecodeError::Truncated(Self::BODY_LEN, body.len()));
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[Self::UNIQUE_NO_OFFSET..][..4].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[Self::ACT_INDEX_OFFSET..][..2].try_into().unwrap()),
        })
    }
}

pin_s2c_offset!(
    ItemTradeReq::UNIQUE_NO_OFFSET,
    req_lsb::UNIQUE_NO,
    "GP_SERV_COMMAND_ITEM_TRADE_REQ.UniqueNo"
);
pin_s2c_offset!(
    ItemTradeReq::ACT_INDEX_OFFSET,
    req_lsb::ACT_INDEX,
    "GP_SERV_COMMAND_ITEM_TRADE_REQ.ActIndex"
);

/// s2c 0x022 ITEM_TRADE_RES Kind (GP_ITEM_TRADE_RES_KIND, 0x022_item_trade_res.h).
/// Serialize: it rides `AgentEvent::TradeRes` across the session bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TradeResKind {
    /// The target accepted; both windows open.
    Start,
    /// A side cancelled the whole trade.
    Cancell,
    /// A side pressed confirm (Make).
    Make,
    /// Un-confirm — never observed being needed by LSB (debug-logged only).
    MakeCancel,
    ErrEtc,
    ErrNoSearchYou,
    ErrNowReq,
    /// Both sides get this when a stale partner had to be cleaned out first
    /// (0x032_trade_req.cpp process).
    ErrYouTrade,
    ErrLogout,
    /// The trade committed and closed.
    End,
}

impl TradeResKind {
    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Start,
            1 => Self::Cancell,
            2 => Self::Make,
            3 => Self::MakeCancel,
            4 => Self::ErrEtc,
            5 => Self::ErrNoSearchYou,
            6 => Self::ErrNowReq,
            7 => Self::ErrYouTrade,
            8 => Self::ErrLogout,
            9 => Self::End,
            _ => return None,
        })
    }
}

/// s2c 0x022 ITEM_TRADE_RES: a trade state change naming the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTradeRes {
    pub unique_no: u32,
    pub kind: TradeResKind,
    pub act_index: u16,
}

impl ItemTradeRes {
    const BODY_LEN: usize = res_lsb::SIZE;
    const UNIQUE_NO_OFFSET: usize = 0x0;
    const KIND_OFFSET: usize = 0x4;
    const ACT_INDEX_OFFSET: usize = 0x8;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::BODY_LEN {
            return Err(DecodeError::Truncated(Self::BODY_LEN, body.len()));
        }
        let kind_raw = u32::from_le_bytes(body[Self::KIND_OFFSET..][..4].try_into().unwrap());
        // The enum is closed (0..=9) in LSB; an unknown code means the server grew a
        // state we must not guess through, so it stays a truncation-free hard error.
        let kind = TradeResKind::from_u32(kind_raw)
            .ok_or(DecodeError::UnknownDiscriminant((kind_raw & 0xFF) as u8))?;
        Ok(Self {
            unique_no: u32::from_le_bytes(body[Self::UNIQUE_NO_OFFSET..][..4].try_into().unwrap()),
            kind,
            act_index: u16::from_le_bytes(body[Self::ACT_INDEX_OFFSET..][..2].try_into().unwrap()),
        })
    }
}

pin_s2c_offset!(
    ItemTradeRes::UNIQUE_NO_OFFSET,
    res_lsb::UNIQUE_NO,
    "GP_SERV_COMMAND_ITEM_TRADE_RES.UniqueNo"
);
pin_s2c_offset!(
    ItemTradeRes::KIND_OFFSET,
    res_lsb::KIND,
    "GP_SERV_COMMAND_ITEM_TRADE_RES.Kind"
);
pin_s2c_offset!(
    ItemTradeRes::ACT_INDEX_OFFSET,
    res_lsb::ACT_INDEX,
    "GP_SERV_COMMAND_ITEM_TRADE_RES.ActIndex"
);

/// One slot row of the partner's offer — s2c 0x023 ITEM_TRADE_LIST. The packet is a
/// per-slot view (player_trade.cpp pushSlotView), one send per changed TradeIndex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTradeList {
    /// Offered quantity; meaningful even for the gil slot (slot 0).
    pub item_num: u32,
    /// The window's refresh counter — echoed by c2s TRADE_RES (vault main_menu.md §25:
    /// retail keeps it at [[0x4DFD98]+0x19518] and stamps confirm with it).
    pub trade_counter: u16,
    /// 0xFFFF means empty slot or gil (disambiguate with `item_num == 0`).
    pub item_no: u16,
    pub item_free_space_num: u8,
    /// Trade slot index 0..=8.
    pub trade_index: u8,
    pub attr: [u8; list_lsb::ATTR_LEN],
}

impl ItemTradeList {
    const BODY_LEN: usize = list_lsb::SIZE;
    const ITEM_NUM_OFFSET: usize = 0x0;
    const TRADE_COUNTER_OFFSET: usize = 0x4;
    const ITEM_NO_OFFSET: usize = 0x6;
    const ITEM_FREE_SPACE_NUM_OFFSET: usize = 0x8;
    const TRADE_INDEX_OFFSET: usize = 0x9;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::BODY_LEN {
            return Err(DecodeError::Truncated(Self::BODY_LEN, body.len()));
        }
        let mut attr = [0u8; list_lsb::ATTR_LEN];
        attr.copy_from_slice(&body[list_lsb::ATTR..list_lsb::ATTR + list_lsb::ATTR_LEN]);
        Ok(Self {
            item_num: u32::from_le_bytes(body[Self::ITEM_NUM_OFFSET..][..4].try_into().unwrap()),
            trade_counter: u16::from_le_bytes(
                body[Self::TRADE_COUNTER_OFFSET..][..2].try_into().unwrap(),
            ),
            item_no: u16::from_le_bytes(body[Self::ITEM_NO_OFFSET..][..2].try_into().unwrap()),
            item_free_space_num: body[Self::ITEM_FREE_SPACE_NUM_OFFSET],
            trade_index: body[Self::TRADE_INDEX_OFFSET],
            attr,
        })
    }
}

pin_s2c_offset!(
    ItemTradeList::ITEM_NUM_OFFSET,
    list_lsb::ITEM_NUM,
    "GP_SERV_COMMAND_ITEM_TRADE_LIST.ItemNum"
);
pin_s2c_offset!(
    ItemTradeList::TRADE_COUNTER_OFFSET,
    list_lsb::TRADE_COUNTER,
    "GP_SERV_COMMAND_ITEM_TRADE_LIST.TradeCounter"
);
pin_s2c_offset!(
    ItemTradeList::ITEM_NO_OFFSET,
    list_lsb::ITEM_NO,
    "GP_SERV_COMMAND_ITEM_TRADE_LIST.ItemNo"
);
pin_s2c_offset!(
    ItemTradeList::ITEM_FREE_SPACE_NUM_OFFSET,
    list_lsb::ITEM_FREE_SPACE_NUM,
    "GP_SERV_COMMAND_ITEM_TRADE_LIST.ItemFreeSpaceNum"
);
pin_s2c_offset!(
    ItemTradeList::TRADE_INDEX_OFFSET,
    list_lsb::TRADE_INDEX,
    "GP_SERV_COMMAND_ITEM_TRADE_LIST.TradeIndex"
);

/// One slot row of MY offer — s2c 0x025 ITEM_TRADE_MYLIST (server echo after our
/// c2s TRADE_LIST lands in setSlot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTradeMyList {
    pub item_num: u32,
    /// 0xFFFF with `item_num == 0` clears the slot (ctor leaves ItemNum at 0);
    /// 0xFFFF with a quantity is the gil slot's view.
    pub item_no: u16,
    pub trade_index: u8,
    /// The bag slot staged for this offer row.
    pub item_index: u8,
}

impl ItemTradeMyList {
    const BODY_LEN: usize = mylist_lsb::SIZE;
    const ITEM_NUM_OFFSET: usize = 0x0;
    const ITEM_NO_OFFSET: usize = 0x4;
    const TRADE_INDEX_OFFSET: usize = 0x6;
    const ITEM_INDEX_OFFSET: usize = 0x7;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::BODY_LEN {
            return Err(DecodeError::Truncated(Self::BODY_LEN, body.len()));
        }
        Ok(Self {
            item_num: u32::from_le_bytes(body[Self::ITEM_NUM_OFFSET..][..4].try_into().unwrap()),
            item_no: u16::from_le_bytes(body[Self::ITEM_NO_OFFSET..][..2].try_into().unwrap()),
            trade_index: body[Self::TRADE_INDEX_OFFSET],
            item_index: body[Self::ITEM_INDEX_OFFSET],
        })
    }
}

pin_s2c_offset!(
    ItemTradeMyList::ITEM_NUM_OFFSET,
    mylist_lsb::ITEM_NUM,
    "GP_SERV_COMMAND_ITEM_TRADE_MYLIST.ItemNum"
);
pin_s2c_offset!(
    ItemTradeMyList::ITEM_NO_OFFSET,
    mylist_lsb::ITEM_NO,
    "GP_SERV_COMMAND_ITEM_TRADE_MYLIST.ItemNo"
);
pin_s2c_offset!(
    ItemTradeMyList::TRADE_INDEX_OFFSET,
    mylist_lsb::TRADE_INDEX,
    "GP_SERV_COMMAND_ITEM_TRADE_MYLIST.TradeIndex"
);
pin_s2c_offset!(
    ItemTradeMyList::ITEM_INDEX_OFFSET,
    mylist_lsb::ITEM_INDEX,
    "GP_SERV_COMMAND_ITEM_TRADE_MYLIST.ItemIndex"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_trade_req_decodes_lsb_layout() {
        // PacketData per 0x021_item_trade_req.h: UniqueNo, ActIndex, padding.
        let mut body = vec![0u8; ItemTradeReq::BODY_LEN];
        body[0..4].copy_from_slice(&0x00A1_0002u32.to_le_bytes());
        body[4..6].copy_from_slice(&0x001Au16.to_le_bytes());
        let req = ItemTradeReq::decode(&body).unwrap();
        assert_eq!(req.unique_no, 0x00A1_0002);
        assert_eq!(req.act_index, 0x1A);
        assert!(matches!(
            ItemTradeReq::decode(&body[..7]),
            Err(DecodeError::Truncated(8, 7))
        ));
    }

    #[test]
    fn item_trade_res_decodes_kinds_and_rejects_unknown() {
        // PacketData per 0x022_item_trade_res.h: Kind is uint32_t-backed.
        let mut body = vec![0u8; ItemTradeRes::BODY_LEN];
        body[0..4].copy_from_slice(&7u32.to_le_bytes());
        body[4..8].copy_from_slice(&2u32.to_le_bytes());
        body[8..10].copy_from_slice(&0x00FFu16.to_le_bytes());
        let res = ItemTradeRes::decode(&body).unwrap();
        assert_eq!(res.unique_no, 7);
        assert_eq!(res.kind, TradeResKind::Make);
        assert_eq!(res.act_index, 0xFF);

        for (raw, want) in [
            (0u32, TradeResKind::Start),
            (1, TradeResKind::Cancell),
            (9, TradeResKind::End),
        ] {
            body[4..8].copy_from_slice(&raw.to_le_bytes());
            assert_eq!(ItemTradeRes::decode(&body).unwrap().kind, want);
        }

        body[4..8].copy_from_slice(&10u32.to_le_bytes());
        assert!(matches!(
            ItemTradeRes::decode(&body),
            Err(DecodeError::UnknownDiscriminant(_))
        ));
    }

    #[test]
    fn item_trade_list_decodes_partner_slot_view() {
        // 34 used bytes of the 0x24 body per 0x023_item_trade_list.h.
        let mut body = vec![0u8; ItemTradeList::BODY_LEN];
        body[0..4].copy_from_slice(&12u32.to_le_bytes());
        body[4..6].copy_from_slice(&3u16.to_le_bytes());
        body[6..8].copy_from_slice(&4212u16.to_le_bytes());
        body[8] = 5;
        body[9] = 2;
        body[10] = 0xA5;
        let row = ItemTradeList::decode(&body).unwrap();
        assert_eq!(row.item_num, 12);
        assert_eq!(row.trade_counter, 3);
        assert_eq!(row.item_no, 4212);
        assert_eq!(row.item_free_space_num, 5);
        assert_eq!(row.trade_index, 2);
        assert_eq!(row.attr[0], 0xA5);
        // Gil view: sentinel id with a quantity (canReceive sums those).
        body[6..8].copy_from_slice(&TRADE_GIL_ITEM_NO.to_le_bytes());
        body[9] = 0;
        let gil = ItemTradeList::decode(&body).unwrap();
        assert_eq!(gil.trade_index, 0);
        assert_eq!(gil.item_no, TRADE_GIL_ITEM_NO);
        assert_ne!(gil.item_num, 0, "a gil view carries the offered amount");
    }

    #[test]
    fn item_trade_mylist_decodes_echo_and_clear() {
        let mut body = vec![0u8; ItemTradeMyList::BODY_LEN];
        body[0..4].copy_from_slice(&99u32.to_le_bytes());
        body[4..6].copy_from_slice(&1234u16.to_le_bytes());
        body[6] = 3;
        body[7] = 9;
        let echo = ItemTradeMyList::decode(&body).unwrap();
        assert_eq!(echo.item_num, 99);
        assert_eq!(echo.item_no, 1234);
        assert_eq!(echo.trade_index, 3);
        assert_eq!(echo.item_index, 9);

        // The clear view LSB's ctor writes: sentinel ItemNo, everything else zero.
        let mut cleared = vec![0u8; ItemTradeMyList::BODY_LEN];
        cleared[4..6].copy_from_slice(&TRADE_GIL_ITEM_NO.to_le_bytes());
        let off = ItemTradeMyList::decode(&cleared).unwrap();
        assert_eq!(off.item_no, TRADE_GIL_ITEM_NO);
        assert_eq!(off.item_num, 0);
        assert_eq!(off.trade_index, 0);
    }
}
