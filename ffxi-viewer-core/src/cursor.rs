//! Custom mouse cursor: replaces the OS arrow with an in-app sprite that
//! changes shape depending on what's under the pointer.
//!
//! Implementation uses Bevy's `CursorIcon::Custom` — the cursor is drawn
//! by the OS compositor (winit forwards the image to AppKit / Win32 /
//! Wayland), so it tracks pointer motion at native zero-lag instead of
//! lagging a frame behind a UI sprite. The `bevy_window` crate has
//! `custom_cursor` in its default feature set, so no extra Cargo
//! configuration is needed.
//!
//! Three states, priority `Rotate > Hand > Arrow`:
//! - `Arrow`: default, over empty world / chrome.
//! - `Hand`: over a selectable target — a `WorldEntity` capsule, a
//!   `Button` UI node, etc.
//! - `Rotate`: while RMB-drag-rotating the camera.
//!
//! Per the operator's choice, the cursor is **never hidden** — including
//! during camera drag, which swaps to `Rotate` instead.
//!
//! ## Why procedural sprites, not PNG assets
//!
//! `vendor/game-files/SquareEnix/PlayOnlineViewer/.../cursor.png` is encrypted
//! (POLViewer XOR asset format, header `d6 7c b4 cc`). The HXUI addon
//! cursors (`vendor/game-files/_addons/HXUI/assets/cursors/`) are real PNGs but
//! are third-party addon assets. Authoring small in-tree bitmaps as
//! `&[&str]` ASCII art keeps the cursor under our control, makes diffs
//! reviewable, and avoids any vendor/license question. Each character
//! maps to one alpha+color: `.` transparent, `o` black outline,
//! `X` white fill.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorIcon, CustomCursor, CustomCursorImage, PrimaryWindow};

use crate::mouse::MousePointer;
use crate::picking::HoveredEntity;

/// Per-frame cursor look. Set by writers (entity-hover, UI-hover,
/// camera-drag) and consumed by the OS-cursor applier. Priority is
/// resolved in [`resolve_cursor_style_system`]: highest-priority
/// requester wins.
///
/// `Default` is `Arrow` — the resting state with nothing under the cursor.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Arrow,
    Hand,
    Rotate,
}

/// Per-frame requests from each writer system. The resolver picks the
/// highest-priority `Some(_)` and writes the final [`CursorStyle`].
/// `Rotate` outranks `Hand` outranks `Arrow` (which is the resting
/// default when no writer requests anything).
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorRequests {
    pub rotate: bool,
    pub hand: bool,
}

/// Cached cursor images + per-style hotspot offsets (in image pixels
/// from the image top-left). Built once at startup from the ASCII-art
/// bitmaps below.
#[derive(Resource)]
pub struct CursorAssets {
    pub arrow: Handle<Image>,
    pub hand: Handle<Image>,
    pub rotate: Handle<Image>,
    pub arrow_hotspot: (u16, u16),
    pub hand_hotspot: (u16, u16),
    pub rotate_hotspot: (u16, u16),
}

pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CursorStyle>()
            .init_resource::<CursorRequests>()
            .add_systems(Startup, build_cursor_assets)
            .add_systems(
                Update,
                (
                    rotate_writer_system,
                    entity_hover_writer_system,
                    ui_hover_writer_system,
                    resolve_cursor_style_system,
                    apply_cursor_icon_system,
                )
                    .chain(),
            );
    }
}

/// Build the three cursor images from ASCII art and stash them as a
/// [`CursorAssets`] resource. Runs once at `Startup`.
fn build_cursor_assets(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let arrow = images.add(rasterize(ARROW_24));
    let hand = images.add(rasterize(HAND_24));
    let rotate = images.add(rasterize(ROTATE_24));
    commands.insert_resource(CursorAssets {
        arrow,
        hand,
        rotate,
        arrow_hotspot: (1, 1),
        hand_hotspot: (8, 1),
        rotate_hotspot: (12, 12),
    });
}

/// Camera-drag writer: while either mouse button is held (retail accepts
/// LMB or RMB for camera rotate), request `Rotate`. Both Chase and
/// FirstPerson modes use the same gate.
fn rotate_writer_system(pointer: Res<MousePointer>, mut req: ResMut<CursorRequests>) {
    req.rotate = pointer.left || pointer.right;
}

/// Entity-hover writer: when an in-world entity is under the cursor,
/// request `Hand`. Reads [`HoveredEntity`] (updated by
/// `crate::picking::update_hovered_entity_system`).
fn entity_hover_writer_system(hovered: Res<HoveredEntity>, mut req: ResMut<CursorRequests>) {
    if hovered.id.is_some() {
        req.hand = true;
    }
}

/// UI-hover writer: when any UI node with an `Interaction` component is
/// currently `Hovered` or `Pressed`, request `Hand`. Bevy UI's
/// `Interaction` automatically tracks pointer state for any node that
/// has the component, so adding `Button` (or just `Interaction`) to a
/// menu row is enough to get a Hand cursor over it for free.
fn ui_hover_writer_system(interactions: Query<&Interaction>, mut req: ResMut<CursorRequests>) {
    for i in &interactions {
        if matches!(i, Interaction::Hovered | Interaction::Pressed) {
            req.hand = true;
            return;
        }
    }
}

/// Collapse per-frame requests into the final [`CursorStyle`] by priority,
/// then clear the request flags ready for the next frame's writers.
fn resolve_cursor_style_system(mut style: ResMut<CursorStyle>, mut req: ResMut<CursorRequests>) {
    let want = if req.rotate {
        CursorStyle::Rotate
    } else if req.hand {
        CursorStyle::Hand
    } else {
        CursorStyle::Arrow
    };
    if *style != want {
        *style = want;
    }
    req.rotate = false;
    req.hand = false;
}

/// When [`CursorStyle`] changes, insert/update the `CursorIcon` component
/// on the primary window entity. winit applies it via the OS native
/// cursor API (NSCursor on macOS, SetCursor on Win32, wl_pointer on
/// Wayland) so motion is zero-lag — the OS compositor draws the cursor.
fn apply_cursor_icon_system(
    style: Res<CursorStyle>,
    assets: Option<Res<CursorAssets>>,
    window_q: Query<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !style.is_changed() {
        return;
    }
    let Some(assets) = assets else {
        return;
    };
    let Ok(window) = window_q.single() else {
        return;
    };
    let (handle, hotspot) = match *style {
        CursorStyle::Arrow => (assets.arrow.clone(), assets.arrow_hotspot),
        CursorStyle::Hand => (assets.hand.clone(), assets.hand_hotspot),
        CursorStyle::Rotate => (assets.rotate.clone(), assets.rotate_hotspot),
    };
    commands
        .entity(window)
        .insert(CursorIcon::Custom(CustomCursor::Image(CustomCursorImage {
            handle,
            texture_atlas: None,
            flip_x: false,
            flip_y: false,
            rect: None,
            hotspot,
        })));
}

/// Convert an ASCII-art cursor bitmap into an RGBA `Image`.
///
/// Character key:
/// - `.` or space: transparent
/// - `o`: black outline (#000 opaque)
/// - `X`: white fill (#FFF opaque)
///
/// Panics if rows have uneven length — that's a programmer error in the
/// const definition, caught at startup with a clear message.
fn rasterize(rows: &[&str]) -> Image {
    let height = rows.len() as u32;
    assert!(height > 0, "cursor bitmap must have at least one row");
    let width = rows[0].chars().count() as u32;
    for (i, row) in rows.iter().enumerate() {
        let len = row.chars().count() as u32;
        assert_eq!(
            len, width,
            "cursor bitmap row {i} width {len} != expected {width}"
        );
    }
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for row in rows {
        for ch in row.chars() {
            let (r, g, b, a) = match ch {
                'o' => (0, 0, 0, 255),
                'X' => (255, 255, 255, 255),
                _ => (0, 0, 0, 0),
            };
            data.extend_from_slice(&[r, g, b, a]);
        }
    }
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// 24×24 arrow cursor — classic Windows-style up-left arrow. Hotspot at
/// the tip (1, 1).
#[rustfmt::skip]
const ARROW_24: &[&str] = &[
    "o.......................",
    "oo......................",
    "oXo.....................",
    "oXXo....................",
    "oXXXo...................",
    "oXXXXo..................",
    "oXXXXXo.................",
    "oXXXXXXo................",
    "oXXXXXXXo...............",
    "oXXXXXXXXo..............",
    "oXXXXXXXXXo.............",
    "oXXXXXXXXXXo............",
    "oXXXXXXXXXXXo...........",
    "oXXXXXXXXo..............",
    "oXXoXXXXo...............",
    "oXo.oXXXo...............",
    "oo...oXXXo..............",
    "o.....oXXXo.............",
    ".......oXXXo............",
    "........oXXo............",
    ".........ooo............",
    "........................",
    "........................",
    "........................",
];

/// 24×24 pointing hand cursor — vertical index finger above a fist.
/// Hotspot at the fingertip (8, 1).
#[rustfmt::skip]
const HAND_24: &[&str] = &[
    "........ooo.............",
    "........oXo.............",
    "........oXo.............",
    "........oXo.............",
    "........oXo.............",
    "........oXoooo..........",
    "........oXoXXoooo.......",
    "........oXoXXoXXoooo....",
    "........oXoXXoXXoXXoo...",
    "........oXoXXoXXoXXoXo..",
    ".....oooXXXXXXXXXXXXXo..",
    "....oXXoXXXXXXXXXXXXXo..",
    "....oXXXXXXXXXXXXXXXXo..",
    ".....oXXXXXXXXXXXXXXXo..",
    ".....oXXXXXXXXXXXXXXXo..",
    "......oXXXXXXXXXXXXXXo..",
    ".......oXXXXXXXXXXXXo...",
    ".......oXXXXXXXXXXXXo...",
    ".......oXXXXXXXXXXXXo...",
    "........oooooooooooo....",
    "........................",
    "........................",
    "........................",
    "........................",
];

/// 24×24 rotate cursor — circular double-arrow indicating camera
/// rotation. Hotspot at the center (12, 12) since the gesture pivots
/// around the cursor point.
#[rustfmt::skip]
const ROTATE_24: &[&str] = &[
    "........................",
    ".........oooo...........",
    ".......ooXXXXoo.........",
    "......oXXXoooXXo........",
    ".....oXXo....oXXo.......",
    "....oXXo......oXXo......",
    "....oXo........oXo......",
    "...oXo..........oo......",
    "...oXo..................",
    "...oXo..................",
    "...oXo..................",
    "...oXo......ooo.........",
    "....oXo....oXXXo........",
    "....oXo...oXXXXXo.......",
    ".....oXoooXXXXXXXo......",
    "......oXXXXXXXXXXo......",
    ".......oXXXXXXXXo.......",
    "........oooXoo..........",
    "..........oo............",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_three_bitmaps_are_24_square() {
        for (name, rows) in [
            ("arrow", ARROW_24),
            ("hand", HAND_24),
            ("rotate", ROTATE_24),
        ] {
            assert_eq!(rows.len(), 24, "{name}: expected 24 rows");
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(
                    row.chars().count(),
                    24,
                    "{name} row {i} not 24 chars: {row:?}"
                );
            }
        }
    }

    #[test]
    fn rasterize_produces_rgba_buffer() {
        let img = rasterize(ARROW_24);
        let size = img.texture_descriptor.size;
        assert_eq!(size.width, 24);
        assert_eq!(size.height, 24);
        let bytes = img.data.as_ref().expect("rasterized image has data").len();
        assert_eq!(bytes, 24 * 24 * 4);
    }

    #[test]
    fn priority_rotate_beats_hand_beats_arrow() {
        let mut req = CursorRequests {
            rotate: true,
            hand: true,
        };
        let want = if req.rotate {
            CursorStyle::Rotate
        } else if req.hand {
            CursorStyle::Hand
        } else {
            CursorStyle::Arrow
        };
        assert_eq!(want, CursorStyle::Rotate);
        req.rotate = false;
        let want = if req.rotate {
            CursorStyle::Rotate
        } else if req.hand {
            CursorStyle::Hand
        } else {
            CursorStyle::Arrow
        };
        assert_eq!(want, CursorStyle::Hand);
        req.hand = false;
        let want = if req.rotate {
            CursorStyle::Rotate
        } else if req.hand {
            CursorStyle::Hand
        } else {
            CursorStyle::Arrow
        };
        assert_eq!(want, CursorStyle::Arrow);
    }
}
