//! NPC shop HUD panel. Phase 1 (read-only listing).
//!
//! Reads `SceneState.snapshot.shop`. When `Some`, draws a center-screen
//! list of `ShopItem` rows (price + item id) and the offset_index header.
//! Hidden via `Display::None` when no shop is open.
//!
//! # Why no item names
//!
//! Item names live in client-side `item.dat` files keyed by item id; we
//! don't ship those. For phase 1 the operator sees a numeric id and a
//! gil price — enough to cross-reference with a wiki or pick a row by
//! position. A follow-up can scrape `item_basic` / equivalent to add a
//! local id→name table.
//!
//! Phase 2 (operator buying) will: add a cursor (Up/Down to select),
//! Enter dispatches `AgentCommand::ShopBuy { shop_no, shop_index, qty }`,
//! Esc closes the shop via `EndEvent`. The data path's already in place
//! — only the input-handling layer is missing.

use bevy::prelude::*;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct ShopPanel;

#[derive(Component)]
pub struct ShopHeader;

#[derive(Component)]
pub struct ShopBody;

const PANEL_WIDTH_PX: f32 = 360.0;

pub fn spawn_shop_panel(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            ShopPanel,
            Node {
                position_type: PositionType::Absolute,
                // Slightly to the right of the dialog panel so a shop
                // event (which usually opens after a dialog choice) doesn't
                // sit directly on top of the dialog HUD.
                top: Val::Percent(35.0),
                left: Val::Percent(60.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                ShopHeader,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
            p.spawn((
                ShopBody,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
}

pub fn update_shop_panel_system(
    state: Res<SceneState>,
    mut panel_q: Query<&mut Node, With<ShopPanel>>,
    mut header_q: Query<&mut Text, (With<ShopHeader>, Without<ShopBody>)>,
    mut body_q: Query<&mut Text, (With<ShopBody>, Without<ShopHeader>)>,
) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut panel_node) = panel_q.single_mut() else {
        return;
    };
    let Some(shop) = state.snapshot.shop.as_ref() else {
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };
    if shop.items.is_empty() {
        // 0x03C with all rows zeroed out (or before SHOP_LIST has arrived
        // in a multi-packet flow) — keep hidden so we don't flash an
        // empty window.
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    }

    if panel_node.display == Display::None {
        panel_node.display = Display::Flex;
    }

    if let Ok(mut text) = header_q.single_mut() {
        let want = format!(
            "Shop  ({} items)  offset={}",
            shop.items.len(),
            shop.offset_index
        );
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = body_q.single_mut() {
        let want = format_rows(&shop.items);
        if **text != want {
            **text = want;
        }
    }
}

/// Compose the rows into a multi-line string. We render at most ~16 rows
/// inline to keep the panel compact; longer shops get a tail elision.
/// Phase 2 will add cursor + scroll, but keeping the readable cap here
/// covers >99% of vanilla shops.
fn format_rows(items: &[ffxi_viewer_wire::ShopItem]) -> String {
    const MAX: usize = 16;
    let mut out = String::new();
    for (i, it) in items.iter().take(MAX).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Format the id-with-prefix as a single unit before width-padding
        // so the `#` stays glued to the digits (`#4096`, not `# 4096`).
        let id = format!("#{}", it.item_no);
        out.push_str(&format!(
            "{:>2}  {:>6}  {:>7} gil",
            it.shop_index, id, it.price
        ));
    }
    if items.len() > MAX {
        out.push_str(&format!("\n… +{} more", items.len() - MAX));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::ShopItem;

    fn item(idx: u8, item_no: u16, price: u32) -> ShopItem {
        ShopItem {
            price,
            item_no,
            shop_index: idx,
            skill: 0,
            guild_info: 0,
        }
    }

    #[test]
    fn rows_format_aligns_columns() {
        let rows = format_rows(&[item(0, 4096, 100), item(1, 256, 99999)]);
        // The `#` prefix must stay glued to the item id (no whitespace
        // injected by width-padding) so an operator can scan the column.
        assert!(rows.contains("#4096"));
        assert!(rows.contains("#256"));
        assert!(rows.contains("100 gil"));
        assert!(rows.contains("99999 gil"));
        // Row 0 then row 1, separated by a newline.
        assert_eq!(rows.lines().count(), 2);
    }

    #[test]
    fn long_lists_get_tail_elision() {
        let items: Vec<ShopItem> = (0..20)
            .map(|i| item(i as u8, 1000 + i as u16, 100))
            .collect();
        let rows = format_rows(&items);
        assert!(rows.contains("+4 more"));
    }
}
