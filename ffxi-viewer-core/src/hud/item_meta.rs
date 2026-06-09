//! Render-time *composition* layer for item detail panels.
//!
//! FFXI item metadata is two-tier:
//!
//! - **Static** (name, description, icon, slot / job / race restriction,
//!   level, rare/ex flags, max charges, base recast): sourced from retail
//!   client DATs, parsed by `ffxi-dat`'s `item_dat` module (owned by a
//!   separate feature agent). Represented here by [`ItemStatic`].
//! - **Dynamic** (uses remaining, current recast/cooldown, equipped state,
//!   current quantity): from server packets, surfaced through
//!   [`ffxi_viewer_wire::SceneSnapshot`].
//!
//! [`compose_item_detail`] is the *single* composer that joins the two,
//! so every leaf view — the Items list, the Trade window, `/check` — shares
//! one code path, mirroring how `status_ribbon` composes the icon sheet
//! plus live `status_icons`.
//!
//! Pure functions only; no Bevy systems.
//!
//! ## Foundation note on [`ItemStatic`]
//!
//! The authoritative static type will be `ffxi_dat::item_dat::ItemStatic`,
//! created by the item-data feature agent. To keep this foundation crate
//! compiling *before* that module exists, [`ItemStatic`] is defined here as
//! the agreed shape. When `ffxi-dat::item_dat` lands, this becomes a
//! re-export / `From` bridge of the DAT type — the field set is identical
//! by design, so downstream `ItemDetail` consumers don't change.

use ffxi_viewer_wire::SceneSnapshot;

/// Static, install-invariant item metadata. Shape mirrors the planned
/// `ffxi_dat::item_dat::ItemStatic`. `icon` is `None` in the foundation
/// stub; the DAT parser fills it from the embedded 32bpp graphic via the
/// existing `map_image::parse_graphic` path.
#[derive(Debug, Clone, Default)]
pub struct ItemStatic {
    pub name: String,
    pub description: String,
    /// Equippable-slot bitmask (FFXI `SLOTTYPE` bits). 0 = not equippable.
    pub slot_mask: u32,
    /// Job bitmask (LSB job ids). 0 = all / none-specific.
    pub jobs_mask: u32,
    /// Race bitmask. 0 = all.
    pub races_mask: u16,
    pub level: u16,
    /// Raw item flags word (rare/ex/enchant/etc).
    pub flags: u16,
    /// Maximum charges for an enchanted item; `None` for non-charge items.
    pub max_charges: Option<u8>,
    /// Base recast in seconds for an enchanted item; `None` when N/A.
    pub recast_base: Option<u16>,
    // NOTE: `icon: Option<ffxi_dat::map_image::GraphicImage>` is added by
    // the DAT bridge. Omitted from the foundation stub to avoid pulling a
    // non-Clone-heavy image into every detail composition before the
    // parser exists.
}

/// The composed, render-ready detail for one item: its static DAT facts
/// joined with the live per-slot dynamic state from the snapshot.
#[derive(Debug, Clone, Default)]
pub struct ItemDetail {
    /// Static metadata; `None` when no DAT install is reachable (the HUD
    /// then degrades to the LSB-scraped label-only fallback).
    pub static_: Option<ItemStatic>,
    /// Enchantment charges left, if the server has reported them.
    pub charges_remaining: Option<u8>,
    /// `(remaining_s, total_s)` recast, if currently on cooldown.
    pub recast: Option<(u16, u16)>,
    /// Whether this item is currently equipped in any slot.
    pub equipped: bool,
    /// Current stack quantity from the inventory mirror.
    pub quantity: u32,
}

/// Compose the full detail for `item_no` from a static-data source `dat`
/// plus live `snapshot` state. This is the one place the two tiers join.
///
/// `dat` is the resolved [`ItemStatic`] for the item (the DAT lookup is
/// performed by the caller, who owns the `DatRoot` handle — keeping this
/// composer free of I/O and trivially unit-testable). Pass `None` when no
/// install is reachable.
///
/// Foundation impl reads the *currently-existing* snapshot fields:
/// `quantity` and `equipped` are resolved now; `charges_remaining` and
/// `recast` are left `None` until the additive snapshot fields
/// (`InventoryItem.charges_remaining`, `snapshot.item_recast`) land in
/// wiring. The signature is the stable contract the leaf views depend on.
pub fn compose_item_detail(
    item_no: u16,
    snapshot: &SceneSnapshot,
    dat: Option<ItemStatic>,
) -> ItemDetail {
    let quantity = snapshot
        .inventory_main
        .iter()
        .filter(|s| s.item_no == item_no)
        .map(|s| s.quantity)
        .sum();

    let equipped = snapshot.equipped.contains(&Some(item_no));

    ItemDetail {
        static_: dat,
        // Dynamic charge / recast state attaches once the additive
        // snapshot fields exist (item-data feature agent, wiring stage).
        charges_remaining: None,
        recast: None,
        equipped,
        quantity,
    }
}
