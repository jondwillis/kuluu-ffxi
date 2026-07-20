//! Item flags scraped from LSB's `item_basic.sql` `flags` column.
//!
//! Bit values mirror `SET @FLAG_*` in vendor/server/sql/item_basic.sql:115-132
//! (and `ItemFlag` in the server source). Only the bits the client needs so
//! far get named constants; the raw word is available via [`lookup`].

include!(concat!(env!("OUT_DIR"), "/item_flags_table.rs"));

/// @FLAG_CAN_SEND_ACCT — deliverable to a character on the same account even
/// when @FLAG_NODELIVERY is set (server enforces the account match).
pub const CAN_SEND_ACCT: u32 = 0x00010;
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
/// dboxutils.cpp:147): NoDelivery blocks staging unless the item also carries
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chocobo_bedding_is_account_bound() {
        // item 1: @FLAG_MYSTERY_BOX | @FLAG_CAN_SEND_ACCT | @FLAG_NOAUCTION |
        // @FLAG_NODELIVERY | @FLAG_EX (item_basic.sql).
        assert_eq!(lookup(1), 0x4 | 0x10 | 0x40 | 0x2000 | 0x4000);
        assert!(deliverable(1), "CanSendAccount overrides NoDelivery");
        assert!(account_bound(1));
    }

    #[test]
    fn simple_bed_is_freely_deliverable() {
        // item 2: @FLAG_MYSTERY_BOX | @FLAG_INSCRIBABLE.
        assert_eq!(lookup(2), 0x4 | 0x20);
        assert!(deliverable(2));
        assert!(!account_bound(2));
    }

    #[test]
    fn unknown_item_has_no_flags() {
        assert_eq!(lookup(u16::MAX), 0);
        assert!(deliverable(u16::MAX));
    }
}
