use bevy::prelude::*;
use bevy::ui::ComputedNode;

use crate::hud::style;

/// Membership in the bottom-right HUD column (self HUD, party roster, target
/// panel). Panels stack upward from the bottom in ascending `order`; a panel
/// with `Display::None` takes no space and the ones above it close the gap.
///
/// This exists so no panel has to know another's height. Each one is measured
/// from its laid-out `ComputedNode`, which is exact and tracks content changes
/// (party size, longer names) that a hardcoded estimate cannot.
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ColumnPanel {
    pub order: u8,
}

impl ColumnPanel {
    pub const SELF_HUD: Self = Self { order: 0 };
    pub const ROSTER: Self = Self { order: 1 };
    pub const TARGET: Self = Self { order: 2 };
}

/// `ComputedNode::size` is physical pixels; `Node::bottom` is logical.
fn logical_height(computed: &ComputedNode) -> f32 {
    computed.size().y * computed.inverse_scale_factor()
}

pub fn layout_panel_column_system(
    mut panels: Query<(Entity, &ColumnPanel, &mut Node, &ComputedNode)>,
    mut last_seen: Local<std::collections::HashMap<Entity, f32>>,
) {
    let mut stack: Vec<_> = panels.iter_mut().collect();
    stack.sort_by_key(|(_, slot, _, _)| **slot);

    // ONE-FRAME HEIGHT CONFIRM. With the render-scale composite active,
    // `ComputedNode` can transiently mismeasure while camera targets
    // re-resolve (physical size and inverse_scale_factor land a frame apart),
    // and trusting each frame's reading restacked the whole column every
    // frame -- the "bouncing bottoms" at any non-default UI scale. A height
    // must repeat (within half a pixel) on two consecutive frames before the
    // column accepts it; a genuine content change lands one frame late,
    // which is invisible, and a flip-flopping reading never moves anything.
    let mut heights: Vec<f32> = Vec::with_capacity(stack.len());
    for (e, _, node, computed) in stack.iter() {
        if node.display == Display::None {
            heights.push(0.0);
            continue;
        }
        let h = logical_height(computed);
        // A panel shown this frame has not been laid out yet. Hold the previous
        // frame's stacking rather than collapsing everything above it onto zero.
        if h <= 0.0 {
            return;
        }
        let prev = last_seen.insert(*e, h);
        match prev {
            None => heights.push(h), // first sighting
            Some(p) if (p - h).abs() <= 0.5 => heights.push(h),
            Some(_) => return, // unconfirmed: hold
        }
    }

    let mut bottom = style::PANEL_COLUMN_BOTTOM_PX;
    for (i, (_, _, node, _)) in stack.iter_mut().enumerate() {
        if node.display == Display::None {
            continue;
        }
        let want = Val::Px(bottom);
        if node.bottom != want {
            node.bottom = want;
        }
        bottom += heights[i] + style::PANEL_COLUMN_GAP_PX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_order_is_self_then_roster_then_target() {
        let mut slots = [
            ColumnPanel::TARGET,
            ColumnPanel::SELF_HUD,
            ColumnPanel::ROSTER,
        ];
        slots.sort();
        assert_eq!(
            slots,
            [
                ColumnPanel::SELF_HUD,
                ColumnPanel::ROSTER,
                ColumnPanel::TARGET
            ]
        );
    }

    fn column_app() -> App {
        let mut app = App::new();
        app.add_systems(Update, layout_panel_column_system);
        app
    }

    fn spawn_panel(app: &mut App, slot: ColumnPanel, height: f32, display: Display) -> Entity {
        let computed = ComputedNode {
            size: Vec2::new(style::PANEL_WIDTH_PX, height),
            inverse_scale_factor: 1.0,
            ..default()
        };
        app.world_mut()
            .spawn((
                slot,
                Node {
                    display,
                    ..default()
                },
                computed,
            ))
            .id()
    }

    fn bottom_of(app: &App, e: Entity) -> Val {
        app.world().get::<Node>(e).unwrap().bottom
    }

    #[test]
    fn panels_stack_upward_from_the_column_bottom() {
        let mut app = column_app();
        let self_hud = spawn_panel(&mut app, ColumnPanel::SELF_HUD, 90.0, Display::Flex);
        let roster = spawn_panel(&mut app, ColumnPanel::ROSTER, 100.0, Display::Flex);
        let target = spawn_panel(&mut app, ColumnPanel::TARGET, 60.0, Display::Flex);
        app.update();

        let gap = style::PANEL_COLUMN_GAP_PX;
        let base = style::PANEL_COLUMN_BOTTOM_PX;
        assert_eq!(bottom_of(&app, self_hud), Val::Px(base));
        assert_eq!(bottom_of(&app, roster), Val::Px(base + 90.0 + gap));
        assert_eq!(
            bottom_of(&app, target),
            Val::Px(base + 90.0 + gap + 100.0 + gap)
        );
    }

    #[test]
    fn hidden_panel_takes_no_space() {
        let mut app = column_app();
        spawn_panel(&mut app, ColumnPanel::SELF_HUD, 90.0, Display::Flex);
        spawn_panel(&mut app, ColumnPanel::ROSTER, 100.0, Display::None);
        let target = spawn_panel(&mut app, ColumnPanel::TARGET, 60.0, Display::Flex);
        app.update();

        assert_eq!(
            bottom_of(&app, target),
            Val::Px(style::PANEL_COLUMN_BOTTOM_PX + 90.0 + style::PANEL_COLUMN_GAP_PX),
            "a hidden roster must not reserve its height"
        );
    }

    #[test]
    fn unmeasured_panel_holds_the_previous_layout() {
        let mut app = column_app();
        spawn_panel(&mut app, ColumnPanel::SELF_HUD, 0.0, Display::Flex);
        let target = spawn_panel(&mut app, ColumnPanel::TARGET, 60.0, Display::Flex);
        app.update();

        assert_eq!(
            bottom_of(&app, target),
            Val::Auto,
            "an unmeasured panel below must not collapse the ones above it onto the base"
        );
    }
}
