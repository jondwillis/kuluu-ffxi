use bevy::prelude::*;

use crate::hud::style::{self, theme};
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
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            p.spawn((
                ShopHeader,
                Text::new(""),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            p.spawn((
                ShopBody,
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TEXT),
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

fn format_rows(items: &[ffxi_viewer_wire::ShopItem]) -> String {
    const MAX: usize = 16;
    let mut out = String::new();
    for (i, it) in items.iter().take(MAX).enumerate() {
        if i > 0 {
            out.push('\n');
        }

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

        assert!(rows.contains("#4096"));
        assert!(rows.contains("#256"));
        assert!(rows.contains("100 gil"));
        assert!(rows.contains("99999 gil"));

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
