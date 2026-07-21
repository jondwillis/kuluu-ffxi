//! Status-effect flags scraped from LSB's `status_effects.sql` `flags` column.
//!
//! Bit values mirror `SET @FLAG_*` in vendor/server/sql/status_effects.sql and
//! `EFFECT` in vendor/server/src/map/status_effect.h. The client keys its buff
//! icons and the buff-cancel packet (0x0F1) on the effect id, so this table is
//! consumed by icon id — LSB assigns icon == effect id by default.

include!(concat!(env!("OUT_DIR"), "/status_effect_flags_table.rs"));

/// EFFECTFLAG_NO_CANCEL — "CAN NOT CLICK IT OFF IN CLIENT"
/// (vendor/server/src/map/status_effect.h:69). Retail's client hides the cancel
/// affordance for these; LSB's 0x0F1 handler does NOT re-check it
/// (vendor/server/src/map/packets/c2s/0x0f1_buffcancel.cpp `// TODO`), so the
/// client is the only gate — a cancel we send for a NO_CANCEL buff would be
/// wrongly honored.
pub const NO_CANCEL: u32 = 0x0080_0000;

/// The `flags` word for effect/icon `id`; effects absent from the sparse table
/// carry 0 (all bits clear).
pub fn flags(id: u16) -> u32 {
    STATUS_EFFECT_FLAGS
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| STATUS_EFFECT_FLAGS[i].1)
        .unwrap_or(0)
}

/// Whether the player may click this buff off in the status window. Mirrors
/// retail: cancelable unless the effect carries `NO_CANCEL`.
pub fn is_cancelable(icon: u16) -> bool {
    icon != 0 && flags(icon) & NO_CANCEL == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_cancel_debuffs_are_not_cancelable() {
        // weakness(1), sleep(2), poison(3), paralysis(4) all carry @FLAG_NO_CANCEL.
        for icon in [1u16, 2, 3, 4] {
            assert!(!is_cancelable(icon), "icon {icon} must be non-cancelable");
        }
    }

    #[test]
    fn common_buffs_are_cancelable() {
        // Protect(40)/Shell(41) are dispelable but player-cancelable.
        assert!(is_cancelable(40));
        assert!(is_cancelable(41));
    }

    #[test]
    fn ko_icon_zero_is_never_cancelable() {
        assert!(!is_cancelable(0));
    }
}
