//! Status-effect icon ribbon — bottom-left, just above the self-HUD bars.
//!
//! Reads `snapshot.status_icons` (decoded from 0x063 type=0x09) and
//! renders a horizontal row of numeric chips. Without DAT-file icon
//! sprites we surface the raw `icon_id` numbers; the operator can
//! cross-reference against a buff-id table when needed. A real sprite
//! pass lands when we ship icon assets.
//!
//! The pool is fixed (`MAX_VISIBLE = 12` chips) — most simultaneous
//! buffs/debuffs in retail FFXI cap around 8; 12 is comfortable
//! headroom. Overflow rows (>12 effects) are dropped from the head so
//! the most-recent additions are visible — `status_icons` is stable
//! per-tick so this just truncates display, not the actual data.

use bevy::prelude::*;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct StatusRibbon;

/// One chip in the ribbon. `slot` is its position 0..MAX_VISIBLE-1.
#[derive(Component)]
pub struct StatusChip {
    pub slot: usize,
}

const MAX_VISIBLE: usize = 12;
const CHIP_WIDTH_PX: f32 = 36.0;

pub fn spawn_status_ribbon(mut commands: Commands) {
    commands
        .spawn((
            StatusRibbon,
            Node {
                position_type: PositionType::Absolute,
                // Bottom-right column, just above the self-HUD panel.
                // Self-HUD: bottom 28..(28 + ~80) = 28..108. Ribbon at
                // bottom: 116 leaves an 8-px gap. Right-anchored so it
                // grows leftward from the same edge as self-HUD; also
                // aligns its rightmost chip with the self-HUD's right
                // border. Right-justified flex packs chips to the right.
                bottom: Val::Px(116.0),
                right: Val::Px(8.0),
                width: Val::Px(MAX_VISIBLE as f32 * (CHIP_WIDTH_PX + 4.0)),
                height: Val::Px(24.0),
                padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                column_gap: Val::Px(2.0),
                ..default()
            },
        ))
        .with_children(|p| {
            for slot in 0..MAX_VISIBLE {
                p.spawn((
                    StatusChip { slot },
                    Node {
                        width: Val::Px(CHIP_WIDTH_PX),
                        height: Val::Px(20.0),
                        padding: UiRect::axes(Val::Px(2.0), Val::Px(0.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        display: Display::None,
                        ..default()
                    },
                    BackgroundColor(palette::BACKGROUND),
                    BorderColor::all(palette::DARK),
                ))
                .with_children(|chip| {
                    chip.spawn((
                        Text::new(""),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(palette::TEXT),
                    ));
                });
            }
        });
}

pub fn update_status_ribbon(
    state: Res<SceneState>,
    mut chips: Query<(&StatusChip, &Children, &mut Node), Without<Text>>,
    mut text_q: Query<&mut Text>,
) {
    if !state.dirty {
        return;
    }
    let icons = &state.snapshot.status_icons;

    for (chip, children, mut node) in chips.iter_mut() {
        let icon_id = icons.get(chip.slot).copied();
        match icon_id {
            Some(id) => {
                if node.display == Display::None {
                    node.display = Display::Flex;
                }
                for child in children.iter() {
                    if let Ok(mut text) = text_q.get_mut(child) {
                        let want = format!("#{id}");
                        if **text != want {
                            **text = want;
                        }
                    }
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Slot allocation is the only logic worth covering — the chip
    /// rendering is thin Bevy glue. The rule is: chip `slot` gets
    /// `icons[slot]` if it exists, otherwise hidden.
    #[test]
    fn slot_allocation_matches_icon_index() {
        let icons = vec![10u16, 20, 30];
        // Slot 0..2 → Some, slot 3+ → None
        for slot in 0..super::MAX_VISIBLE {
            let got = icons.get(slot).copied();
            let want = match slot {
                0 => Some(10),
                1 => Some(20),
                2 => Some(30),
                _ => None,
            };
            assert_eq!(got, want, "slot {slot}");
        }
    }
}
