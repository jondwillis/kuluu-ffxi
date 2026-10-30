use super::*;

/// The ceiling a seller may ask for one unit of merchandise: LSB validates the
/// price of c2s 0x10A against exactly this range, and retail's own `Set Prices`
/// spinner prints its cap as `/99,999,999 G`
/// (vendor/server/src/map/packets/c2s/0x10a_bazaar_itemset.cpp validate).
pub const BAZAAR_PRICE_MAX: u32 = 99_999_999;

/// What survives of a bazaar comment: c2s 0x0DE carries a fixed `uint8_t[123]` and LSB's
/// handler stores `sizeof(sInspectMessage) - 3` bytes of it verbatim to `char_stats`
/// (vendor/server/src/map/packets/c2s/0x0de_inspect_message.cpp
/// GP_CLI_COMMAND_INSPECT_MESSAGE::process).
pub const BAZAAR_COMMENT_MAX_BYTES: usize = 120;

/// One priced row of a browsed bazaar (s2c 0x105 GP_SERV_COMMAND_BAZAAR_LIST).
/// The server pushes one packet per priced LOC_INVENTORY slot when we open the
/// bazaar, then re-pushes the single affected row after every purchase — so
/// consumers merge by `index` rather than accumulating
/// (vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process).
/// vendor/server/src/map/packets/s2c/0x105_bazaar_list.h GP_SERV_COMMAND_BAZAAR_LIST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BazaarListItem {
    /// Seller's asking price per unit, before tax (`CItem::getCharPrice`).
    pub price: u32,
    pub quantity: u32,
    /// Zone tax in hundredths of a percent: the buyer pays
    /// `price * qty * (10000 + tax_rate) / 10000`
    /// (vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process totalPrice).
    pub tax_rate: u16,
    pub item_no: u16,
    /// Seller-side LOC_INVENTORY slot; the id c2s 0x106 buys by.
    pub index: u8,
}

impl BazaarListItem {
    const PRICE_OFFSET: usize = 0;
    const QUANTITY_OFFSET: usize = 4;
    const TAX_OFFSET: usize = 8;
    const ITEM_NO_OFFSET: usize = 10;
    const INDEX_OFFSET: usize = 12;
    pub const SIZE: usize = Self::INDEX_OFFSET + 1;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let rd32 = |o: usize| u32::from_le_bytes(body[o..o + 4].try_into().unwrap());
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        Ok(Self {
            price: rd32(Self::PRICE_OFFSET),
            quantity: rd32(Self::QUANTITY_OFFSET),
            tax_rate: rd16(Self::TAX_OFFSET),
            item_no: rd16(Self::ITEM_NO_OFFSET),
            index: body[Self::INDEX_OFFSET],
        })
    }

    /// Whether the row still offers anything. The post-purchase refresh carries
    /// the emptied slot with a zero price, which is how the seller-side "no
    /// longer for sale" state reaches the buyer.
    pub fn for_sale(&self) -> bool {
        self.price != 0 && self.quantity != 0
    }
}

/// `GP_BAZAAR_BUY_STATE`, vendor/server/src/map/packets/s2c/0x106_bazaar_buy.h.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BazaarBuyState {
    Ok,
    Err,
    End,
}

impl BazaarBuyState {
    fn from_u32(raw: u32) -> Result<Self, DecodeError> {
        match raw {
            0 => Ok(Self::Ok),
            1 => Ok(Self::Err),
            2 => Ok(Self::End),
            other => Err(DecodeError::UnknownDiscriminant(other as u8)),
        }
    }
}

/// Answer to our c2s 0x106 purchase attempt; `seller` is the bazaar owner.
/// vendor/server/src/map/packets/s2c/0x106_bazaar_buy.h GP_SERV_COMMAND_BAZAAR_BUY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BazaarBuy {
    pub state: BazaarBuyState,
    pub seller: String,
}

impl BazaarBuy {
    const STATE_OFFSET: usize = 0;
    const NAME_OFFSET: usize = 4;
    pub const SIZE: usize = Self::NAME_OFFSET + NAME_LEN;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            state: BazaarBuyState::from_u32(u32::from_le_bytes(
                body[Self::STATE_OFFSET..Self::STATE_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ))?,
            seller: name_at(body, Self::NAME_OFFSET),
        })
    }
}

/// The bazaar we were browsing emptied or closed (s2c 0x107).
/// vendor/server/src/map/packets/s2c/0x107_bazaar_close.h GP_SERV_COMMAND_BAZAAR_CLOSE padding00.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BazaarClose {
    pub seller: String,
}

impl BazaarClose {
    pub const SIZE: usize = NAME_LEN;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            seller: name_at(body, 0),
        })
    }
}

/// Another customer bought a row out from under us while we browse (s2c 0x109);
/// a refreshed 0x105 row for `index` follows. `buyer` is the purchasing PC
/// (vendor/server/src/map/packets/s2c/0x109_bazaar_sell.cpp GP_SERV_COMMAND_BAZAAR_SELL::GP_SERV_COMMAND_BAZAAR_SELL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BazaarSell {
    pub buyer_id: u32,
    pub quantity: u32,
    pub buyer: String,
    pub index: u8,
}

impl BazaarSell {
    const UNIQUE_NO_OFFSET: usize = 0;
    const QUANTITY_OFFSET: usize = 4;
    const NAME_OFFSET: usize = 12;
    const INDEX_OFFSET: usize = Self::NAME_OFFSET + NAME_LEN;
    pub const SIZE: usize = Self::INDEX_OFFSET + 1;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            buyer_id: u32::from_le_bytes(
                body[Self::UNIQUE_NO_OFFSET..Self::UNIQUE_NO_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ),
            quantity: u32::from_le_bytes(
                body[Self::QUANTITY_OFFSET..Self::QUANTITY_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ),
            buyer: name_at(body, Self::NAME_OFFSET),
            index: body[Self::INDEX_OFFSET],
        })
    }
}

const NAME_LEN: usize = 16;

fn name_at(body: &[u8], offset: usize) -> String {
    let bytes = &body[offset..offset + NAME_LEN];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_row_reads_price_quantity_tax_and_slot() {
        let mut buf = vec![0u8; 40];
        buf[0..4].copy_from_slice(&12_000u32.to_le_bytes());
        buf[4..8].copy_from_slice(&3u32.to_le_bytes());
        buf[8..10].copy_from_slice(&500u16.to_le_bytes());
        buf[10..12].copy_from_slice(&17440u16.to_le_bytes());
        buf[12] = 7;
        let row = BazaarListItem::decode(&buf).expect("decode");
        assert_eq!(
            row,
            BazaarListItem {
                price: 12_000,
                quantity: 3,
                tax_rate: 500,
                item_no: 17440,
                index: 7,
            }
        );
        assert!(row.for_sale());
    }

    #[test]
    fn a_zero_priced_refresh_row_is_no_longer_for_sale() {
        let mut buf = vec![0u8; BazaarListItem::SIZE];
        buf[12] = 7;
        assert!(!BazaarListItem::decode(&buf).expect("decode").for_sale());
    }

    #[test]
    fn buy_states_map_to_the_lsb_enum() {
        let mut buf = vec![0u8; BazaarBuy::SIZE];
        buf[4..9].copy_from_slice(b"Aliya");
        for (raw, want) in [
            (0u32, BazaarBuyState::Ok),
            (1, BazaarBuyState::Err),
            (2, BazaarBuyState::End),
        ] {
            buf[0..4].copy_from_slice(&raw.to_le_bytes());
            let got = BazaarBuy::decode(&buf).expect("decode");
            assert_eq!(got.state, want);
            assert_eq!(got.seller, "Aliya");
        }
        buf[0..4].copy_from_slice(&3u32.to_le_bytes());
        assert!(matches!(
            BazaarBuy::decode(&buf),
            Err(DecodeError::UnknownDiscriminant(3))
        ));
    }

    #[test]
    fn close_carries_the_seller_name() {
        let mut buf = vec![0u8; 20];
        buf[0..5].copy_from_slice(b"Aliya");
        assert_eq!(
            BazaarClose::decode(&buf).expect("decode").seller,
            "Aliya".to_string()
        );
    }

    #[test]
    fn sell_notice_reads_buyer_slot_and_quantity() {
        let mut buf = vec![0u8; BazaarSell::SIZE];
        buf[0..4].copy_from_slice(&0x0104_00D2u32.to_le_bytes());
        buf[4..8].copy_from_slice(&2u32.to_le_bytes());
        buf[12..17].copy_from_slice(b"Vhaan");
        buf[28] = 5;
        let sell = BazaarSell::decode(&buf).expect("decode");
        assert_eq!(sell.buyer_id, 0x0104_00D2);
        assert_eq!(sell.quantity, 2);
        assert_eq!(sell.buyer, "Vhaan");
        assert_eq!(sell.index, 5);
    }

    #[test]
    fn truncated_bodies_error() {
        assert!(matches!(
            BazaarListItem::decode(&[0u8; BazaarListItem::SIZE - 1]),
            Err(DecodeError::Truncated(n, _)) if n == BazaarListItem::SIZE
        ));
        assert!(matches!(
            BazaarBuy::decode(&[0u8; BazaarBuy::SIZE - 1]),
            Err(DecodeError::Truncated(n, _)) if n == BazaarBuy::SIZE
        ));
        assert!(matches!(
            BazaarSell::decode(&[0u8; BazaarSell::SIZE - 1]),
            Err(DecodeError::Truncated(n, _)) if n == BazaarSell::SIZE
        ));
    }
}
