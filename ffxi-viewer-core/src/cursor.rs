//! Custom mouse cursor: replaces the OS arrow with an in-app cursor that
//! changes shape depending on what's under the pointer.
//!
//! Three states, priority `Rotate > Hand > Arrow`:
//! - `Arrow`: default, over empty world / chrome.
//! - `Hand`: over a selectable target — a `WorldEntity` capsule, a
//!   `Button` UI node, etc.
//! - `Rotate`: while drag-rotating the camera (button held *and* dragged
//!   past the motion threshold — a bare click never triggers it).
//!
//! ## How each state is drawn (and the macOS drag caveat)
//!
//! `Arrow`/`Hand` use Bevy's `CursorIcon::Custom` — the OS compositor draws
//! the cursor (NSCursor / SetCursor / wl_pointer), so it tracks pointer
//! motion at native zero-lag. That path works while the pointer moves
//! freely.
//!
//! It does **not** work during a camera drag on macOS. AppKit applies a
//! window's cursor only through cursor *rects* (`resetCursorRects`), and it
//! stops consulting them while a mouse button is held; winit-0.30 has no
//! `cursorUpdate:` fallback (`macos/view.rs` only implements
//! `resetCursorRects`). So a custom cursor set at drag-start survives
//! exactly one frame, then AppKit reverts to the system arrow. The `Rotate`
//! cursor — whose entire job is to show *during* a button-held drag — can't
//! be delivered by the OS path on macOS.
//!
//! `Rotate` is therefore handled FFXI-retail style: while dragging we lock
//! the pointer (`CursorGrabMode::Locked`; raw `MouseMotion` still drives the
//! camera), hide the OS cursor, and pin a `Rotate` sprite at the lock point
//! via a UI overlay. On release the pointer unlocks and reappears where the
//! drag began. This lock/hide/sprite path ([`apply_rotate_lock_system`]) is
//! native-only — it's compiled out on web, where CSS cursors render
//! correctly during drags and the `CursorIcon::Custom` path covers all three
//! states.
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
use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::GlobalZIndex;
use bevy::window::{CursorIcon, CustomCursor, CustomCursorImage, PrimaryWindow};
#[cfg(not(target_arch = "wasm32"))]
use bevy::window::{CursorGrabMode, CursorOptions};

use crate::mouse::MousePointer;
#[cfg(not(target_arch = "wasm32"))]
use crate::mouse::CursorLockRequest;
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

/// Marker for the UI overlay sprite that stands in for the OS cursor while
/// drag-rotating (see [`apply_rotate_lock_system`]). One persistent entity,
/// spawned hidden at startup and toggled via `Node.display`. Not tagged
/// `InGameEntity` — it's chrome that outlives zone changes.
#[derive(Component)]
pub struct RotateCursorSprite;

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
                    // macOS can't show an OS cursor mid-drag; lock + hide +
                    // pin a sprite instead. Web cursors work during drags,
                    // so this is native-only.
                    #[cfg(not(target_arch = "wasm32"))]
                    apply_rotate_lock_system,
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

    // Persistent pinned-rotate sprite, hidden until a camera drag locks the
    // pointer. Absolute-positioned UI overlay above all HUD chrome; ignores
    // picking so it never intercepts hover/clicks.
    commands.spawn((
        RotateCursorSprite,
        ImageNode::new(rotate.clone()),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(24.0),
            height: Val::Px(24.0),
            display: Display::None,
            ..default()
        },
        GlobalZIndex(i32::MAX),
        Pickable::IGNORE,
    ));

    commands.insert_resource(CursorAssets {
        arrow,
        hand,
        rotate,
        arrow_hotspot: (1, 1),
        hand_hotspot: (8, 1),
        rotate_hotspot: (12, 12),
    });
}

/// Camera-drag writer: request `Rotate` while either mouse button is held
/// *and* has dragged past the motion threshold (retail accepts LMB or RMB
/// for camera rotate; both Chase and FirstPerson use the same gate).
///
/// Gating on the `*_dragged` flags — not the bare button — keeps a click
/// (press + release, no motion) from flashing the rotate cursor or briefly
/// locking the pointer, which matters now that `Rotate` locks/hides the OS
/// cursor on native. The flags persist through the drag (and survive a
/// mid-drag pause) until the button is next pressed, so the lock holds.
fn rotate_writer_system(pointer: Res<MousePointer>, mut req: ResMut<CursorRequests>) {
    req.rotate =
        (pointer.left && pointer.left_dragged) || (pointer.right && pointer.right_dragged);
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

/// Native-only `Rotate` handling: lock + hide the OS pointer and pin a
/// `Rotate` sprite at the lock point for the duration of a camera drag.
///
/// Why this exists: macOS (and X11) stop honoring a window's cursor rects
/// while a mouse button is held, so the OS `Rotate` cursor set by
/// [`apply_cursor_icon_system`] can't display during a drag (see the module
/// docs). Locking the pointer freezes it in place while raw `MouseMotion`
/// keeps driving [`crate::mouse::mouse_camera_system`]; hiding it removes the
/// half-frame system-arrow flicker; the sprite shows the operator a rotate
/// affordance pinned where the drag began. On release the pointer unlocks
/// and the OS cursor reappears at that frozen position.
///
/// Sole owner of the primary window's [`CursorOptions`] (grab mode +
/// visibility). The free-look lock request ([`CursorLockRequest`], F8) is
/// unioned into the grab decision so it keeps working, but only a drag hides
/// the cursor — free-look historically kept it visible.
#[cfg(not(target_arch = "wasm32"))]
fn apply_rotate_lock_system(
    style: Res<CursorStyle>,
    lock_request: Res<CursorLockRequest>,
    pointer: Res<MousePointer>,
    assets: Option<Res<CursorAssets>>,
    win_q: Query<&Window, With<PrimaryWindow>>,
    mut opts_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut sprite_q: Query<&mut Node, With<RotateCursorSprite>>,
    mut prev_rotating: Local<bool>,
    mut lock_point: Local<Vec2>,
) {
    let Some(assets) = assets else {
        return;
    };
    // Gate the lock on focus: a blur (Cmd-Tab, etc.) can swallow the
    // button-release event, leaving a phantom held button. Without this,
    // that would strand the pointer locked + hidden — an invisible cursor
    // over other apps. Losing focus collapses `rotating`, so the pointer
    // unlocks and reappears immediately.
    let focused = win_q.single().map(|w| w.focused).unwrap_or(false);
    let rotating = matches!(*style, CursorStyle::Rotate) && focused;

    // Latch the pin position on the rising edge. Once the pointer locks,
    // `cursor_pos` stops updating (no more `CursorMoved`), so this value
    // stays put for the whole drag even as raw motion rotates the camera.
    if rotating && !*prev_rotating {
        *lock_point = pointer.cursor_pos.unwrap_or(Vec2::ZERO);
    }
    *prev_rotating = rotating;

    if let Ok(mut opts) = opts_q.single_mut() {
        let want_grab = if lock_request.locked || rotating {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if opts.grab_mode != want_grab {
            opts.grab_mode = want_grab;
        }
        // Hide only during a rotate-drag; a free-look lock keeps the pointer
        // visible, matching the prior free-look behavior.
        let want_visible = !rotating;
        if opts.visible != want_visible {
            opts.visible = want_visible;
        }
    }

    if let Ok(mut node) = sprite_q.single_mut() {
        if rotating {
            let (hx, hy) = assets.rotate_hotspot;
            let left = Val::Px(lock_point.x - hx as f32);
            let top = Val::Px(lock_point.y - hy as f32);
            if node.display != Display::Flex {
                node.display = Display::Flex;
            }
            if node.left != left {
                node.left = left;
            }
            if node.top != top {
                node.top = top;
            }
        } else if node.display != Display::None {
            node.display = Display::None;
        }
    }
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
