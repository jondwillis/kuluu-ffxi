use std::time::{SystemTime, UNIX_EPOCH};

use ffxi_viewer_wire::{InventoryItem, SceneSnapshot};

#[derive(Debug, Clone, Default)]
pub struct ItemStatic {
    pub name: String,
    pub description: String,

    pub slot_mask: u32,

    pub jobs_mask: u32,

    pub races_mask: u16,
    pub level: u16,

    pub flags: u16,

    pub max_charges: Option<u8>,

    pub recast_base: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct ItemDetail {
    pub static_: Option<ItemStatic>,

    pub charges_remaining: Option<u8>,

    /// `(remaining_secs, base_secs)` recast: live countdown over the static
    /// reuse delay. Both in whole seconds (a 24h enchant is 86400 > u16).
    pub recast: Option<(u32, u32)>,

    pub equipped: bool,

    pub quantity: u32,
}

/// Current Vana'diel timestamp (Earth seconds since the vanadiel epoch) from
/// wall-clock, matching the server's `next_use_vana_ts` frame.
pub fn now_vana_ts() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(ffxi_proto::vana_time::VANA_EPOCH_UNIX) as u32
}

/// A charged item is unusable when it has no charges left or its recast has not
/// elapsed. Non-charged items (`charges_remaining` is `None`) are never greyed
/// by this predicate. Job/level/zone/status gating is server-side and
/// deliberately not reproduced here (see kuluu-ng3o notes).
pub fn item_unusable(item: &InventoryItem, now_vana: u32) -> bool {
    match item.charges_remaining {
        None => false,
        Some(0) => true,
        Some(_) => item
            .next_use_vana_ts
            .is_some_and(|ts| ts != 0 && ts > now_vana),
    }
}

/// The exact inventory instance at `(container, index)`. Charges/recast are
/// per-instance, so callers with a focused slot resolve it here rather than by
/// item id (two copies of a charged item can be in different states).
pub fn find_slot(snapshot: &SceneSnapshot, container: u8, index: u8) -> Option<&InventoryItem> {
    snapshot
        .containers
        .iter()
        .flat_map(|c| c.items.iter())
        .find(|it| it.container == container && it.index == index)
}

pub fn compose_item_detail(
    item_no: u16,
    focused_slot: Option<(u8, u8)>,
    snapshot: &SceneSnapshot,
    dat: Option<ItemStatic>,
) -> ItemDetail {
    let quantity = snapshot
        .containers
        .iter()
        .flat_map(|c| c.items.iter())
        .filter(|s| s.item_no == item_no)
        .map(|s| s.quantity)
        .sum();

    let equipped = snapshot.equipped.contains(&Some(item_no));

    let slot = focused_slot
        .and_then(|(container, index)| find_slot(snapshot, container, index))
        .or_else(|| {
            snapshot
                .containers
                .iter()
                .flat_map(|c| c.items.iter())
                .find(|s| s.item_no == item_no)
        });

    let charges_remaining = slot.and_then(|s| s.charges_remaining);
    let recast = dat.as_ref().and_then(|d| d.recast_base).map(|base| {
        let now_vana = now_vana_ts();
        let remaining = slot
            .and_then(|s| s.next_use_vana_ts)
            .map(|ts| ts.saturating_sub(now_vana))
            .unwrap_or(0);
        (remaining, base)
    });

    ItemDetail {
        static_: dat,

        charges_remaining,
        recast,
        equipped,
        quantity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charged(charges: Option<u8>, next_use: Option<u32>) -> InventoryItem {
        InventoryItem {
            container: 0,
            index: 0,
            item_no: 4096,
            quantity: 1,
            locked: false,
            charges_remaining: charges,
            next_use_vana_ts: next_use,
        }
    }

    #[test]
    fn non_charged_item_is_never_unusable() {
        assert!(!item_unusable(&charged(None, None), 1000));
    }

    #[test]
    fn empty_charges_is_unusable() {
        assert!(item_unusable(&charged(Some(0), Some(0)), 1000));
    }

    #[test]
    fn charged_item_on_cooldown_is_unusable() {
        assert!(item_unusable(&charged(Some(1), Some(2000)), 1000));
    }

    #[test]
    fn charged_item_ready_is_usable() {
        assert!(!item_unusable(&charged(Some(1), Some(0)), 1000));
        assert!(!item_unusable(&charged(Some(1), Some(500)), 1000));
    }

    #[test]
    fn compose_resolves_the_focused_instance_not_the_first_of_the_id() {
        // Two copies of the same charged item_no in different recast states: a
        // ready one at index 0 and an on-cooldown one at index 1. Focusing the
        // cooldown slot must read ITS charges/recast, not the ready copy's.
        let ready = InventoryItem {
            container: 0,
            index: 0,
            item_no: 4096,
            quantity: 1,
            locked: false,
            charges_remaining: Some(1),
            next_use_vana_ts: Some(0),
        };
        let cooling = InventoryItem {
            container: 0,
            index: 1,
            charges_remaining: Some(0),
            next_use_vana_ts: Some(u32::MAX),
            ..ready
        };
        let snap = SceneSnapshot {
            containers: vec![ffxi_viewer_wire::ContainerView {
                id: 0,
                capacity: 30,
                items: vec![ready, cooling],
            }],
            ..Default::default()
        };
        let dat = Some(ItemStatic {
            max_charges: Some(1),
            recast_base: Some(3600),
            ..Default::default()
        });

        let focused = compose_item_detail(4096, Some((0, 1)), &snap, dat.clone());
        assert_eq!(focused.charges_remaining, Some(0));

        // Without a focused slot it falls back to the first copy of the id.
        let fallback = compose_item_detail(4096, None, &snap, dat);
        assert_eq!(fallback.charges_remaining, Some(1));
    }
}
