//! Retail's Controls-page law: which rows the window draws, which actions one row re-binds together,
//! and what each action's default binding looks like. All of it is table data in the client image —
//! FFXiMain.dll retail-2026-09 `.rdata 0x37A290` (29 records of `u16 action, u16 linked[4]`, read per
//! row by `.text 0x19CF60` and written across the whole link group by `.text 0x19CFA0`), `.data
//! 0x36D6B0` (`u16 slot[3]` per action; `Get .text 0x123670`, `Set .text 0x123680` writes
//! `key | PRIMARY_BINDING_BIT`, `Set2 .text 0x1236B0` writes `key | SECONDARY_BINDING_BIT`) and the
//! per-action context mask table `.rdata 0x36CF60`. Measured on the retail-2026-09 install; vault
//! main_menu.md §59 addendum 25.
//!
//! kuluu's own Controls presets stay a separate surface — this is the shape any per-row rebind has to
//! reproduce, and the reason editing Confirm/Cancel also moves menu navigation.

/// A link slot that means "no further action" (`0xFFFF` in every unused slot of the row table).
pub const NO_LINK: u16 = 0xFFFF;
/// Bit `Set` puts on a binding to mark it as the action's primary key (`.text 0x123680`).
pub const PRIMARY_BINDING_BIT: u16 = 0x1000;
/// Bit `Set2` puts on a secondary binding (`.text 0x1236B0`, used by the page's two extra keys).
pub const SECONDARY_BINDING_BIT: u16 = 0x8000;

/// Menu-mode direction/accept actions, separate from the row actions above: Up `0x2B`, Down `0x2C`,
/// Right `0x2D`, Left `0x2E` (translator FFXiMain.dll retail-2026-09 `.text 0x1AB853`).
pub const MENU_ACTIONS: [u16; 4] = [0x2B, 0x2C, 0x2D, 0x2E];

/// One Controls row: the action its caption names, plus up to four further actions that must be
/// re-bound with it. Row order is the page's display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlRow {
    pub action: u16,
    pub linked: [u16; 4],
}

const fn row(action: u16, linked: [u16; 4]) -> ControlRow {
    ControlRow { action, linked }
}

/// The whole table, in display order (FFXiMain.dll retail-2026-09 `.rdata 0x37A290`, 29 records).
pub const RETAIL_CONTROL_ROWS: [ControlRow; 29] = [
    row(0x3B, [NO_LINK; 4]),
    row(0x39, [NO_LINK; 4]),
    row(0x41, [NO_LINK; 4]),
    row(0x7F, [NO_LINK; 4]),
    row(0x80, [NO_LINK; 4]),
    row(0x9F, [NO_LINK; 4]),
    // Confirm: re-binds the three other accept ids and the menu-mode confirm action.
    row(0x0E, [0x42, 0x8F, 0x91, MENU_CONFIRM]),
    // Cancel: same for the cancel family (menu-mode cancel is the third slot).
    row(0x0F, [0x90, 0x92, MENU_CANCEL, 0x95]),
    row(0x18, [NO_LINK; 4]),
    row(0x78, [0x11, NO_LINK, NO_LINK, NO_LINK]),
    row(0x56, [NO_LINK; 4]),
    row(0x55, [NO_LINK; 4]),
    row(0x45, [NO_LINK; 4]),
    row(0x4F, [0xA4, NO_LINK, NO_LINK, NO_LINK]),
    row(0x50, [0xA5, NO_LINK, NO_LINK, NO_LINK]),
    row(0x81, [NO_LINK; 4]),
    row(0x82, [NO_LINK; 4]),
    row(0x36, [NO_LINK; 4]),
    row(0x40, [0x10, NO_LINK, NO_LINK, NO_LINK]),
    row(0x3D, [NO_LINK; 4]),
    row(0x3F, [NO_LINK; 4]),
    row(0x3C, [NO_LINK; 4]),
    row(0x3A, [NO_LINK; 4]),
    row(0x38, [NO_LINK; 4]),
    row(0x3E, [NO_LINK; 4]),
    row(0x37, [NO_LINK; 4]),
    row(0xAD, [NO_LINK; 4]),
    row(0xAE, [NO_LINK; 4]),
    row(0xB4, [NO_LINK; 4]),
];

/// Menu-mode accept/cancel ids: they are the ones a Confirm/Cancel row pulls along with it.
pub const MENU_CONFIRM: u16 = 0x93;
pub const MENU_CANCEL: u16 = 0x94;

/// One action's live binding triple plus its context mask, as measured from the image (low byte of a
/// slot is the DirectInput scancode; `slot == 0` means the row draws blank).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultBinding {
    pub action: u16,
    pub slots: [u16; 3],
    pub context_mask: u32,
}

const fn binding(action: u16, slots: [u16; 3], context_mask: u32) -> DefaultBinding {
    DefaultBinding {
        action,
        slots,
        context_mask,
    }
}

/// Defaults for every action the Controls page touches (its rows plus their link groups), read from
/// `.data 0x36D6B0` / `.rdata 0x36CF60`. `binding_for` answers them by action.
pub const RETAIL_DEFAULT_BINDINGS: [DefaultBinding; 29] = [
    binding(0x3B, [0x101E, 0, 0], 0xFFFF_FFFF),
    binding(0x39, [0x1017, 0, 0], 0xFFFF_FFFF),
    binding(0x41, [0x1032, 0, 0], 0xFFFF_FFFF),
    binding(0x7F, [0x1024, 0, 0], 0x0000_0000),
    binding(0x80, [0x1011, 0, 0], 0x1012_2290),
    binding(0x9F, [0x1030, 0, 0], 0x0000_0000),
    binding(0x0E, [0x1015, 0x001C, 0x009C], 0xFFFF_0008),
    binding(
        0x0F,
        [0x1031, SECONDARY_BINDING_BIT | ESC_SCANCODE, 0],
        0x003E_003E,
    ),
    binding(0x18, [0x100C, 0x804A, 0], 0x0040_0008),
    binding(0x78, [0x1021, 0x804E, 0], 0x1012_2250),
    binding(0x56, [0x101B, 0x8046, 0], 0x003E_0001),
    binding(0x55, [0x101A, 0x80B7, 0x80B7], 0x0001_FFFF),
    binding(0x45, [0x102C, 0xC0B5, 0], 0x003E_FFFF),
    binding(0x4F, [0x1034, 0xC0C9, 0xC049], 0xFFFF_0004),
    binding(0x50, [0x1033, 0xC0D1, 0xC051], 0x0008_0008),
    // Two row actions ship unbound: slot 0 is zero, so retail draws those rows blank.
    binding(0x81, [0, 0, 0], 0x0000_0000),
    binding(0x82, [0, 0, 0], 0x1012_2330),
    binding(0x36, [0x1012, 0, 0], 0xFFFF_FFFF),
    binding(0x40, [0x102F, 0, 0], 0xFFFF_FFFF),
    binding(0x3D, [0x1023, 0, 0], 0x0004_0004),
    binding(0x3F, [0x102E, 0, 0], 0xFFFF_0004),
    binding(0x3C, [0x101F, 0, 0], 0x0004_FFFF),
    binding(0x3A, [0x1019, 0, 0], 0xFFFF_FFFF),
    binding(0x38, [0x1014, 0, 0], 0xFFFF_FFFF),
    binding(0x3E, [0x1026, 0, 0], 0x0004_0004),
    binding(0x37, [0x1013, 0, 0], 0xFFFF_FFFF),
    binding(0xAD, [0x1016, 0, 0], 0x0000_0000),
    binding(0xAE, [0x102D, 0, 0], 0x1012_23F0),
    binding(0xB4, [0x1010, 0, 0], 0x1012_23D0),
];

/// DirectInput scancode for Enter — the name in Confirm's secondary slot (`.data 0x36D6B0` action `0x0E`, slot 1).
pub const ENTER_SCANCODE: u16 = 0x1C;
/// …and Escape, named by Cancel's secondary slot (action `0x0F`, slot 1) as `key | SECONDARY_BINDING_BIT`.
pub const ESC_SCANCODE: u16 = 0x01;

/// The link group of `action`, when it names one of the page's rows.
pub fn row_links_for(action: u16) -> Option<&'static [u16; 4]> {
    RETAIL_CONTROL_ROWS
        .iter()
        .find(|r| r.action == action)
        .map(|r| &r.linked)
}

/// The default binding of `action` as the image ships it.
pub fn binding_for(action: u16) -> Option<&'static DefaultBinding> {
    RETAIL_DEFAULT_BINDINGS.iter().find(|b| b.action == action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_table_has_no_duplicate_actions() {
        let mut actions: Vec<u16> = RETAIL_CONTROL_ROWS.iter().map(|r| r.action).collect();
        let count = actions.len();
        actions.sort_unstable();
        actions.dedup();
        assert_eq!(actions.len(), count, "29 distinct row actions");
    }

    #[test]
    fn confirm_and_cancel_rows_pull_the_menu_mode_ids_along() {
        assert_eq!(row_links_for(0x0E), Some(&[0x42, 0x8F, 0x91, MENU_CONFIRM]));
        assert_eq!(row_links_for(0x0F), Some(&[0x90, 0x92, MENU_CANCEL, 0x95]));
    }

    #[test]
    fn every_row_action_has_a_measured_default_binding() {
        for r in RETAIL_CONTROL_ROWS.iter() {
            assert!(
                binding_for(r.action).is_some(),
                "action 0x{:02X} missing from the default table",
                r.action
            );
        }
    }

    #[test]
    fn defaults_decode_to_real_keys_and_the_two_unbound_rows() {
        assert_eq!(binding_for(0x0E).unwrap().slots[1], ENTER_SCANCODE);
        let cancel = binding_for(0x0F).unwrap();
        assert_eq!(cancel.slots[1] & !SECONDARY_BINDING_BIT, ESC_SCANCODE);
        for unbound in [0x81u16, 0x82] {
            let b = binding_for(unbound).unwrap();
            assert_eq!(b.slots, [0; 3], "action 0x{unbound:02X} ships unbound");
        }
    }

    #[test]
    fn primary_and_secondary_binding_bits_are_distinct() {
        let confirm = binding_for(0x0E).unwrap();
        assert_ne!(confirm.slots[0] & PRIMARY_BINDING_BIT, 0);
        assert_eq!(confirm.slots[1] & SECONDARY_BINDING_BIT, 0);
    }
}
