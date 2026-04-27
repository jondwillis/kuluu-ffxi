use ffxi_viewer_wire::SceneSnapshot;

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

    pub recast_base: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct ItemDetail {
    pub static_: Option<ItemStatic>,

    pub charges_remaining: Option<u8>,

    pub recast: Option<(u16, u16)>,

    pub equipped: bool,

    pub quantity: u32,
}

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

        charges_remaining: None,
        recast: None,
        equipped,
        quantity,
    }
}
