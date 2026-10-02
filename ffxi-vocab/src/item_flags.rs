//! Item flags scraped from LSB's `item_basic.sql` `flags` column.
//!
//! Bit values mirror `SET @FLAG_*` in vendor/server/sql/item_basic.sql item_basic
//! (and `ItemFlag` in the server source). Only the bits the client needs so
//! far get named constants; the raw word is available via [`lookup`].

include!(concat!(env!("OUT_DIR"), "/item_flags_table.rs"));
include!(concat!(env!("OUT_DIR"), "/item_stack_size_table.rs"));

/// @FLAG_CAN_SEND_ACCT — deliverable to a character on the same account even
/// when @FLAG_NODELIVERY is set (server enforces the account match).
pub const CAN_SEND_ACCT: u32 = 0x00010;
/// @FLAG_NOAUCTION — cannot be listed on the Auction House.
pub const NOAUCTION: u32 = 0x00040;
/// @FLAG_NOSALE — cannot be sold to an NPC shop.
pub const NOSALE: u32 = 0x01000;
/// @FLAG_NODELIVERY — cannot be staged into the delivery box.
pub const NODELIVERY: u32 = 0x02000;
/// @FLAG_EX — cannot be traded.
pub const EX: u32 = 0x04000;
/// @FLAG_RARE — only one may be held.
pub const RARE: u32 = 0x08000;

/// The `flags` word for `id`; items absent from the sparse table carry 0.
pub fn lookup(id: u16) -> u32 {
    ITEM_FLAGS
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| ITEM_FLAGS[i].1)
        .unwrap_or(0)
}

/// Whether the delivery-box send picker should offer this item at all.
///
/// Mirrors dboxutils::AddItemsToBeSent (vendor/server/src/map/utils/
/// dboxutils.cpp dboxutils::AddItemsToBeSent): NoDelivery blocks staging unless the item also carries
/// CanSendAccount — in which case the server still requires the recipient to
/// be on the sender's account, which only it can verify.
pub fn deliverable(id: u16) -> bool {
    let flags = lookup(id);
    flags & NODELIVERY == 0 || flags & CAN_SEND_ACCT != 0
}

/// Whether staging `id` is restricted to same-account recipients.
pub fn account_bound(id: u16) -> bool {
    let flags = lookup(id);
    flags & NODELIVERY != 0 && flags & CAN_SEND_ACCT != 0
}

/// Whether the AH sell picker should offer this item. Mirrors auctionutils
/// SellingItems' NoAuction reject (vendor/server/src/map/utils/auctionutils.cpp).
pub fn auctionable(id: u16) -> bool {
    lookup(id) & NOAUCTION == 0
}

/// Whether the NPC-shop sell picker should offer this item. Mirrors the
/// `!PItem->hasFlag(ItemFlag::NoSale)` guard in
/// vendor/server/src/map/packets/c2s/0x084_shop_sell_req.cpp process, which
/// silently drops the appraisal for a NoSale item.
pub fn sellable(id: u16) -> bool {
    lookup(id) & NOSALE == 0
}

/// How many of `id` fit in one inventory slot (`item_basic.stackSize`). Items
/// absent from the sparse table do not stack. This is the cap a shop purchase
/// is sized against; LSB clamps anything larger
/// (vendor/server/src/map/packets/c2s/0x083_shop_buy.cpp process).
pub fn stack_size(id: u16) -> u8 {
    ITEM_STACK_SIZES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| ITEM_STACK_SIZES[i].1)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // vendor/server/sql/item_basic.sql @FLAG_MYSTERY_BOX / @FLAG_INSCRIBABLE
    const MYSTERY_BOX: u32 = 0x00004;
    const INSCRIBABLE: u32 = 0x00020;

    #[test]
    fn chocobo_bedding_is_account_bound() {
        // item 1: @FLAG_MYSTERY_BOX | @FLAG_CAN_SEND_ACCT | @FLAG_NOAUCTION |
        // @FLAG_NODELIVERY | @FLAG_EX (item_basic.sql).
        assert_eq!(
            lookup(1),
            MYSTERY_BOX | CAN_SEND_ACCT | NOAUCTION | NODELIVERY | EX
        );
        assert!(deliverable(1), "CanSendAccount overrides NoDelivery");
        assert!(account_bound(1));
        assert!(!auctionable(1), "NoAuction blocks the AH sell picker");
    }

    #[test]
    fn simple_bed_is_freely_deliverable() {
        // item 2: @FLAG_MYSTERY_BOX | @FLAG_INSCRIBABLE.
        assert_eq!(lookup(2), MYSTERY_BOX | INSCRIBABLE);
        assert!(deliverable(2));
        assert!(!account_bound(2));
    }

    #[test]
    fn nosale_furnishing_is_kept_out_of_the_shop_sell_picker() {
        // item 7 (gold_bed): @FLAG_INSCRIBABLE | @FLAG_NOAUCTION | @FLAG_NOSALE |
        // @FLAG_NODELIVERY | @FLAG_EX (item_basic.sql).
        assert_eq!(lookup(7) & NOSALE, NOSALE);
        assert!(!sellable(7));
        assert!(sellable(2));
    }

    /// item 4096 (fire crystal) stacks to 12; item 7 (gold_bed) does not stack.
    #[test]
    fn stack_size_comes_from_item_basic() {
        assert_eq!(stack_size(4096), 12);
        assert_eq!(stack_size(7), 1);
        assert_eq!(stack_size(u16::MAX), 1, "unknown ids do not stack");
    }

    #[test]
    fn unknown_item_has_no_flags() {
        assert_eq!(lookup(u16::MAX), 0);
        assert!(deliverable(u16::MAX));
        assert!(sellable(u16::MAX));
    }
}
