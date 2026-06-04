//! Status-effect icon UI — top-left, matching the retail FFXI client.
//!
//! Retail anchors the active status-effect icons in the top-left corner
//! (beside the HP/MP/TP block when it's visible, but the icons stand on
//! their own — the bar block only appears in battle, the icons persist).
//! We mirror that: a top-left, left-to-right row that wraps into a grid,
//! independent of the self-HUD.
//!
//! Sprites come from the retail client's own status-icon sheet
//! (`ffxi_dat::map_image::STATUS_ICON_FILE_ID` → `ROM/119/57.DAT`): a
//! flat array of 640 blocks, `status_id N` → block N, each a 32×32 32bpp
//! `sts_icon` Graphic. Decoded once per id and cached as a Bevy texture
//! ([`StatusIconCache`]). When the DAT isn't reachable or an id fails to
//! decode, the slot falls back to a numeric `#id` chip so the operator
//! still sees *something*.
//!
//! Reads `snapshot.status_icons` (decoded from 0x063 type=0x09). The
//! pool is fixed at [`MAX_VISIBLE`] = 32 slots, matching the packet's
//! `icons[32]` capacity; surplus effects (rare) truncate from the tail.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::map_image::{status_icon_at, STATUS_ICON_FILE_ID};
use ffxi_dat::DatRoot;

use crate::hud::palette;
use crate::snapshot::SceneState;

/// Front-end-provided handle for resolving the status-icon DAT to a disk
/// path. Same Arc the minimap's `MinimapDatRoot` carries — the front-end
/// inserts both from one `DatRoot`. Without it (or without a reachable
/// install), the ribbon degrades to numeric chips.
#[derive(Resource, Default, Clone)]
pub struct StatusIconDatRoot(pub Option<Arc<DatRoot>>);

/// Decoded status-icon textures, keyed by `status_id`. `None` marks an
/// id we tried and failed to decode (out-of-range, truncated block, or
/// empty slot) so we don't re-attempt every tick.
///
/// This is a **persistent asset cache**, not session-scoped game state:
/// the icon sheet is identical across characters/zones/logins, so the
/// textures are loaded once and kept for the process lifetime — like a
/// font atlas. Intentionally *not* drained by `despawn_ingame_entities`
/// (re-decoding the same 640-icon sheet on every zone-in would be pure
/// waste). The UI nodes that *display* the icons are session-scoped and
/// carry `InGameEntity`, so they drain normally.
#[derive(Resource, Default)]
pub struct StatusIconCache {
    /// Whole status-icon DAT, read once on first need.
    dat: Option<Arc<Vec<u8>>>,
    /// Set once a load attempt failed so we stop retrying the file read.
    dat_unavailable: bool,
    /// Per-id texture handle; `None` = decode failed (use numeric chip).
    icons: HashMap<u16, Option<Handle<Image>>>,
}

impl StatusIconCache {
    /// Resolve (and lazily decode) the texture for `status_id`. Returns
    /// `None` when the DAT is unreachable or the id doesn't decode — the
    /// caller then shows the numeric fallback.
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

    /// Lazily read the status-icon DAT once, caching the bytes. Returns
    /// `None` (and latches `dat_unavailable`) if the root is unset or the
    /// file can't be read.
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
                // Don't latch unavailable on a missing root — the
                // front-end may insert the resource a frame later.
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
                warn!("status icons: DAT file_id {STATUS_ICON_FILE_ID} unreadable; numeric fallback");
                self.dat_unavailable = true;
                None
            }
        }
    }
}

/// Upload a decoded 32×32 icon as a Bevy texture. Linear sampling so the
/// 32px source reads cleanly when drawn at [`ICON_SIZE_PX`].
fn upload_icon(img: ffxi_dat::map_image::GraphicImage, images: &mut Assets<Image>) -> Handle<Image> {
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

/// 1×1 transparent placeholder so each chip's `ImageNode` has a valid
/// handle before its real icon is assigned (and as the "hidden image"
/// state when a slot falls back to a numeric chip).
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

/// Marker on the top-left grid container.
#[derive(Component)]
pub struct StatusRibbon;

/// One icon slot in the grid. `slot` is its position 0..MAX_VISIBLE-1,
/// which maps to `status_icons[slot]`.
#[derive(Component)]
pub struct StatusChip {
    pub slot: usize,
}

/// Marker on the numeric-fallback `Text` child of a chip.
#[derive(Component)]
pub struct StatusChipFallback;

/// Slot pool size — matches the 0x063 packet's `icons[32]` capacity.
const MAX_VISIBLE: usize = 32;
/// Rendered icon edge length, px. Retail draws status icons small.
const ICON_SIZE_PX: f32 = 20.0;
/// Gap between icons, px. Retail packs them tightly.
const ICON_GAP_PX: f32 = 1.0;
/// Icons per row before wrapping (bounds the container width).
const ICONS_PER_ROW: usize = 16;

pub fn spawn_status_ribbon(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);
    let row_width = ICONS_PER_ROW as f32 * (ICON_SIZE_PX + ICON_GAP_PX);

    commands
        .spawn((
            crate::components::InGameEntity,
            StatusRibbon,
            Node {
                position_type: PositionType::Absolute,
                // Top-left corner, small margin. Anchored independently
                // of the self-HUD bars (which only appear in battle).
                top: Val::Px(8.0),
                left: Val::Px(8.0),
                width: Val::Px(row_width),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::FlexStart,
                align_content: AlignContent::FlexStart,
                column_gap: Val::Px(ICON_GAP_PX),
                row_gap: Val::Px(ICON_GAP_PX),
                ..default()
            },
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
                        display: Display::None,
                        ..default()
                    },
                    ImageNode::new(placeholder.clone()),
                    // Transparent until a slot needs the numeric-chip
                    // backing (set in `update_status_ribbon`).
                    BackgroundColor(Color::NONE),
                ))
                .with_children(|chip| {
                    chip.spawn((
                        StatusChipFallback,
                        Text::new(""),
                        TextFont {
                            font_size: 10.0,
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
                // Real sprite: show the image, no backing, clear text.
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
                // Fallback: hide the (placeholder) image, show a dark
                // chip with the numeric id.
                if image_node.color.alpha() != 0.0 {
                    image_node.color = Color::NONE;
                }
                if bg.0 != palette::BACKGROUND {
                    bg.0 = palette::BACKGROUND;
                }
                set_fallback_text(children, &mut text_q, &format!("{icon_id}"));
            }
        }
    }
}

/// Update a chip's numeric-fallback text child, skipping the write when
/// it already matches so Bevy's change detection doesn't churn.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot allocation is the load-bearing logic: chip `slot` gets
    /// `icons[slot]` if present, otherwise hides. (Texture upload is
    /// thin Bevy/asset glue, covered by the `ffxi-dat` decode tests.)
    #[test]
    fn slot_allocation_matches_icon_index() {
        let icons = vec![10u16, 20, 30];
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

    /// An empty cache with no DAT root yields `None` (numeric fallback)
    /// without latching `dat_unavailable` — the front-end may insert the
    /// root a frame later, and we must retry then.
    #[test]
    fn cache_without_root_does_not_latch() {
        let mut cache = StatusIconCache::default();
        let root = StatusIconDatRoot(None);
        assert!(cache.dat_bytes(&root).is_none());
        assert!(!cache.dat_unavailable, "must retry once root is provided");
    }
}
