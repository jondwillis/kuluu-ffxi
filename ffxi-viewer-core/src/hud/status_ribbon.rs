use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::map_image::{status_icon_at, STATUS_ICON_FILE_ID};
use ffxi_dat::DatRoot;

use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Resource, Default, Clone)]
pub struct StatusIconDatRoot(pub Option<Arc<DatRoot>>);

#[derive(Resource, Default)]
pub struct StatusIconCache {
    dat: Option<Arc<Vec<u8>>>,

    dat_unavailable: bool,

    icons: HashMap<u16, Option<Handle<Image>>>,
}

impl StatusIconCache {
    fn ensure(
        &mut self,
        status_id: u16,
        dat_root: &StatusIconDatRoot,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(slot) = self.icons.get(&status_id) {
            return slot.clone();
        }
        let handle = self
            .dat_bytes(dat_root)
            .and_then(|bytes| status_icon_at(&bytes, status_id))
            .map(|img| upload_icon(img, images));
        self.icons.insert(status_id, handle.clone());
        handle
    }

    fn dat_bytes(&mut self, dat_root: &StatusIconDatRoot) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = &self.dat {
            return Some(bytes.clone());
        }
        if self.dat_unavailable {
            return None;
        }
        let root = match &dat_root.0 {
            Some(r) => r,
            None => {
                return None;
            }
        };
        let loaded = root
            .resolve(STATUS_ICON_FILE_ID)
            .ok()
            .map(|loc| loc.path_under(root.root()))
            .and_then(|path| std::fs::read(path).ok());
        match loaded {
            Some(bytes) => {
                let arc = Arc::new(bytes);
                self.dat = Some(arc.clone());
                Some(arc)
            }
            None => {
                warn!(
                    "status icons: DAT file_id {STATUS_ICON_FILE_ID} unreadable; numeric fallback"
                );
                self.dat_unavailable = true;
                None
            }
        }
    }
}

fn upload_icon(
    img: ffxi_dat::map_image::GraphicImage,
    images: &mut Assets<Image>,
) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    images.add(image)
}

fn transparent_placeholder(images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

#[derive(Component)]
pub struct StatusRibbon;

#[derive(Component)]
pub struct StatusChip {
    pub slot: usize,
}

#[derive(Component)]
pub struct StatusChipFallback;

#[derive(Component)]
pub struct StatusChipTimer;

const MAX_VISIBLE: usize = 32;

const ICON_SIZE_PX: f32 = 20.0;

const ICON_GAP_PX: f32 = 1.0;

pub const ICONS_PER_ROW: usize = 16;

pub fn spawn_status_ribbon(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);
    let row_width = ICONS_PER_ROW as f32 * (ICON_SIZE_PX + ICON_GAP_PX);

    commands
        .spawn((
            crate::components::InGameEntity,
            StatusRibbon,
            Node {
                position_type: PositionType::Absolute,

                // Below the menu help bar so chips never overlap it when open.
                top: Val::Px(crate::hud::menu_help_bar::BAR_HEIGHT + 6.0),
                left: Val::Px(8.0),
                width: Val::Px(row_width),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::FlexStart,
                align_content: AlignContent::FlexStart,
                column_gap: Val::Px(ICON_GAP_PX),
                row_gap: Val::Px(ICON_GAP_PX),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(Color::NONE),
        ))
        .with_children(|p| {
            for slot in 0..MAX_VISIBLE {
                p.spawn((
                    StatusChip { slot },
                    Node {
                        width: Val::Px(ICON_SIZE_PX),
                        height: Val::Px(ICON_SIZE_PX),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border: UiRect::all(Val::Px(1.0)),
                        display: Display::None,
                        ..default()
                    },
                    ImageNode::new(placeholder.clone()),
                    BackgroundColor(Color::NONE),
                    BorderColor::all(Color::NONE),
                    Interaction::default(),
                ))
                .with_children(|chip| {
                    chip.spawn((
                        StatusChipFallback,
                        Text::new(""),
                        style::text_font(10.0),
                        TextColor(theme::TEXT),
                    ));
                    chip.spawn((
                        StatusChipTimer,
                        Node {
                            position_type: PositionType::Absolute,
                            bottom: Val::Px(-2.0),
                            left: Val::Px(0.0),
                            width: Val::Px(ICON_SIZE_PX),
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        Text::new(""),
                        style::text_font(8.0),
                        TextColor(theme::TITLE),
                    ));
                });
            }
        });
}

pub fn update_status_ribbon(
    state: Res<SceneState>,
    dat_root: Res<StatusIconDatRoot>,
    mut cache: ResMut<StatusIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut chips: Query<(
        &StatusChip,
        &Children,
        &mut Node,
        &mut ImageNode,
        &mut BackgroundColor,
    )>,
    mut text_q: Query<&mut Text, With<StatusChipFallback>>,
) {
    if !state.dirty {
        return;
    }
    let icons = &state.snapshot.status_icons;

    for (chip, children, mut node, mut image_node, mut bg) in chips.iter_mut() {
        let Some(&icon_id) = icons.get(chip.slot) else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        if node.display == Display::None {
            node.display = Display::Flex;
        }

        match cache.ensure(icon_id, &dat_root, &mut images) {
            Some(handle) => {
                if image_node.image != handle {
                    image_node.image = handle;
                }
                if image_node.color != Color::WHITE {
                    image_node.color = Color::WHITE;
                }
                if bg.0 != Color::NONE {
                    bg.0 = Color::NONE;
                }
                set_fallback_text(children, &mut text_q, "");
            }
            None => {
                if image_node.color.alpha() != 0.0 {
                    image_node.color = Color::NONE;
                }
                if bg.0 != theme::CELL_BG {
                    bg.0 = theme::CELL_BG;
                }
                set_fallback_text(children, &mut text_q, &format!("{icon_id}"));
            }
        }
    }
}

/// Highlights the status ribbon while it holds the active-window cursor: a
/// frame border on the ribbon and a cursor border on the selected chip — bright
/// (`CURSOR`) when that buff is player-cancelable, muted otherwise, so the
/// player can see which buffs Confirm will click off (retail's status window).
pub fn update_status_ribbon_selection(
    mode: Res<crate::input_mode::InputMode>,
    state: Res<SceneState>,
    mut ribbon_q: Query<&mut BorderColor, (With<StatusRibbon>, Without<StatusChip>)>,
    mut chips: Query<(&StatusChip, &mut BorderColor), Without<StatusRibbon>>,
) {
    use crate::input_mode::{InputMode, PassiveCursorFocus};

    if !mode.is_changed() && !state.is_changed() {
        return;
    }

    let (focused, cursor) = match &*mode {
        InputMode::PassiveCursor(s) if matches!(s.focus, PassiveCursorFocus::StatusIcons) => {
            (true, s.status_cursor)
        }
        _ => (false, usize::MAX),
    };

    let ribbon_border = if focused {
        theme::FRAME_EDGE
    } else {
        Color::NONE
    };
    for mut border in ribbon_q.iter_mut() {
        if border.left != ribbon_border {
            *border = BorderColor::all(ribbon_border);
        }
    }

    let icons = &state.snapshot.status_icons;
    for (chip, mut border) in chips.iter_mut() {
        let want = if focused && chip.slot == cursor {
            let icon = icons.get(chip.slot).copied().unwrap_or(0);
            if ffxi_proto::status_effects::is_cancelable(icon) {
                theme::CURSOR
            } else {
                theme::MUTED
            }
        } else {
            Color::NONE
        };
        if border.left != want {
            *border = BorderColor::all(want);
        }
    }
}

fn set_fallback_text(
    children: &Children,
    text_q: &mut Query<&mut Text, With<StatusChipFallback>>,
    want: &str,
) {
    for child in children.iter() {
        if let Ok(mut text) = text_q.get_mut(child) {
            if **text != want {
                **text = want.to_string();
            }
        }
    }
}

pub fn update_status_timers(
    state: Res<SceneState>,
    clock: Res<crate::vana_time::VanaClock>,
    chips: Query<(&StatusChip, &Children)>,
    mut timer_q: Query<&mut Text, With<StatusChipTimer>>,
) {
    let now = clock.earth_unix_secs_now() as u32;
    let expiries = &state.snapshot.status_icon_expiries;
    for (chip, children) in chips.iter() {
        let want = expiries
            .get(chip.slot)
            .copied()
            .filter(|&e| e != 0)
            .map(|e| e.saturating_sub(now))
            .filter(|&r| r > 0)
            .map(crate::hud::format_timer)
            .unwrap_or_default();
        for child in children.iter() {
            if let Ok(mut text) = timer_q.get_mut(child) {
                if **text != want {
                    **text = want.clone();
                }
            }
        }
    }
}

/// Enhanced (non-retail) hover tooltip for the status ribbon: shows the buff's
/// name from the scraped status-effect table when the pointer is over a chip.
#[cfg(feature = "enhanced-buff-tooltips")]
pub mod tooltip {
    use super::*;
    use crate::mouse::MousePointer;

    #[derive(Component)]
    pub struct BuffTooltip;

    #[derive(Component)]
    pub struct BuffTooltipText;

    const TOOLTIP_OFFSET_PX: Vec2 = Vec2::new(16.0, 16.0);

    pub fn spawn_buff_tooltip(mut commands: Commands) {
        commands
            .spawn((
                crate::components::InGameEntity,
                BuffTooltip,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(-1000.0),
                    top: Val::Px(-1000.0),
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(theme::FRAME_BG),
                BorderColor::all(theme::FRAME_EDGE),
                ZIndex(i32::MAX - 1),
            ))
            .with_children(|p| {
                p.spawn((
                    BuffTooltipText,
                    Text::new(""),
                    style::text_font(12.0),
                    TextColor(theme::TEXT),
                ));
            });
    }

    pub fn update_buff_tooltip(
        state: Res<SceneState>,
        pointer: Res<MousePointer>,
        chips: Query<(&StatusChip, &Interaction)>,
        mut card_q: Query<&mut Node, With<BuffTooltip>>,
        mut text_q: Query<&mut Text, With<BuffTooltipText>>,
    ) {
        let Ok(mut card) = card_q.single_mut() else {
            return;
        };

        let icons = &state.snapshot.status_icons;
        let hovered_icon = chips
            .iter()
            .find(|(_, i)| matches!(i, Interaction::Hovered | Interaction::Pressed))
            .and_then(|(chip, _)| icons.get(chip.slot).copied());

        let name = hovered_icon.and_then(ffxi_proto::status_names::lookup);
        let Some(name) = name else {
            if card.display != Display::None {
                card.display = Display::None;
            }
            return;
        };

        if card.display == Display::None {
            card.display = Display::Flex;
        }
        if let Some(pos) = pointer.cursor_pos {
            let want_left = Val::Px(pos.x + TOOLTIP_OFFSET_PX.x);
            let want_top = Val::Px(pos.y + TOOLTIP_OFFSET_PX.y);
            if card.left != want_left {
                card.left = want_left;
            }
            if card.top != want_top {
                card.top = want_top;
            }
        }
        if let Ok(mut text) = text_q.single_mut() {
            if text.as_str() != name {
                **text = name.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_allocation_matches_icon_index() {
        let icons = [10u16, 20, 30];
        for slot in 0..MAX_VISIBLE {
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

    #[test]
    fn cache_without_root_does_not_latch() {
        let mut cache = StatusIconCache::default();
        let root = StatusIconDatRoot(None);
        assert!(cache.dat_bytes(&root).is_none());
        assert!(!cache.dat_unavailable, "must retry once root is provided");
    }
}
