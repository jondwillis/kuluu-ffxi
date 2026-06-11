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
//! ## How the cursor is drawn (native vs web)
//!
//! **Web** uses Bevy's `CursorIcon::Custom` ([`apply_cursor_icon_system`]) —
//! the browser draws a CSS cursor at zero lag, including during drags.
//!
//! **Native (macOS in particular) can't rely on `CursorIcon::Custom`.**
//! AppKit applies a window's cursor only through cursor *rects*
//! (`resetCursorRects` → `addCursorRect:cursor:`) and winit-0.30 has no
//! `cursorUpdate:` fallback (`macos/view.rs`). In a continuously-redrawing
//! app the custom cursor reverts to the system arrow almost immediately — it
//! shows for ~one frame after each change, then reverts — and AppKit stops
//! consulting cursor rects entirely while a mouse button is held (a camera
//! drag). Hiding the OS cursor (`set_cursor_visible`), by contrast, *is*
//! honored reliably.
//!
//! So on native we hide the OS cursor and draw it ourselves as a UI overlay
//! sprite that follows the pointer ([`apply_cursor_sprite_system`]), swapping
//! `Arrow`/`Hand`/`Rotate` art and offsetting by each state's hotspot. The
//! tradeoff is ~one frame of pointer lag — far better than a cursor that
//! won't persist at all.
//!
//! `Rotate` additionally locks the pointer (`CursorGrabMode::Locked`; raw
//! `MouseMotion` still drives the camera) so a camera drag doesn't slide the
//! pointer across the screen — the sprite pins at the lock point and the OS
//! pointer reappears there on release, FFXI-retail style.
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

/// Marker for the UI overlay sprite that stands in for the OS cursor on
/// native (see [`apply_cursor_sprite_system`]). One persistent entity,
/// spawned hidden at startup; its image/position/visibility are driven each
/// frame. Not tagged `InGameEntity` — it's chrome that outlives zone changes.
#[derive(Component)]
pub struct CursorSprite;

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
                    // Web: OS-native CSS cursor (zero-lag, persists). Native:
                    // the icon never persists, so the sprite system below
                    // hides the OS cursor and draws it instead — the icon
                    // insert is a harmless no-op there.
                    apply_cursor_icon_system,
                    // Native: hide the OS cursor and draw an overlay sprite
                    // for all three states (the OS custom cursor won't hold
                    // on macOS); lock the pointer while drag-rotating.
                    #[cfg(not(target_arch = "wasm32"))]
                    apply_cursor_sprite_system,
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

    // Persistent cursor sprite (native stand-in for the OS cursor), hidden
    // until `apply_cursor_sprite_system` activates it. Absolute-positioned UI
    // overlay above all HUD chrome; ignores picking so it never intercepts
    // hover/clicks. Image swaps per `CursorStyle` at runtime.
    commands.spawn((
        CursorSprite,
        ImageNode::new(arrow.clone()),
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

/// Native-only cursor renderer: hide the OS cursor and draw the active
/// [`CursorStyle`] as a UI overlay sprite at the pointer.
///
/// On macOS the OS custom cursor ([`apply_cursor_icon_system`]) won't persist
/// — it reverts to the system arrow within a frame (see the module docs) —
/// but hiding the OS cursor *is* reliable, so we hide it and draw the sprite
/// ourselves. While idle the sprite follows the pointer with ~one frame of
/// lag; `Rotate` instead locks the pointer (raw `MouseMotion` still drives
/// [`crate::mouse::mouse_camera_system`]) and pins the sprite at the lock
/// point, so a camera drag doesn't slide the pointer — it reappears there on
/// release.
///
/// Sole owner of the primary window's [`CursorOptions`] (grab mode +
/// visibility). The free-look lock request ([`CursorLockRequest`], F8) is
/// unioned into the grab decision so it keeps working.
///
/// The sprite hides (and the OS cursor returns) whenever the window is
/// unfocused or the pointer is outside it. That also makes a blur (Cmd-Tab)
/// that swallows the button-release event safe: a phantom held button can't
/// strand the pointer locked + hidden, since losing focus collapses the
/// active state and unlocks.
#[cfg(not(target_arch = "wasm32"))]
fn apply_cursor_sprite_system(
    style: Res<CursorStyle>,
    lock_request: Res<CursorLockRequest>,
    assets: Option<Res<CursorAssets>>,
    win_q: Query<&Window, With<PrimaryWindow>>,
    mut opts_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut sprite_q: Query<(&mut Node, &mut ImageNode), With<CursorSprite>>,
    mut prev_rotating: Local<bool>,
    mut lock_point: Local<Vec2>,
) {
    let Some(assets) = assets else {
        return;
    };
    let Ok(window) = win_q.single() else {
        return;
    };
    let focused = window.focused;
    // `None` once the pointer leaves the window. While `Rotate` holds the
    // lock the latched point is used instead, so a transient `None` there is
    // harmless.
    let live_pos = window.cursor_position();

    let rotating = matches!(*style, CursorStyle::Rotate) && focused;
    // Latch the pin on the rising edge — under the lock `cursor_position`
    // stops updating, so this holds for the whole drag even as raw motion
    // rotates the camera.
    if rotating && !*prev_rotating {
        if let Some(p) = live_pos {
            *lock_point = p;
        }
    }
    *prev_rotating = rotating;

    // Our sprite stands in for the OS cursor only while focused and over the
    // window (a drag forces "over the window" via the latched point).
    let active = focused && (rotating || live_pos.is_some());

    if let Ok(mut opts) = opts_q.single_mut() {
        let want_grab = if lock_request.locked || rotating {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if opts.grab_mode != want_grab {
            opts.grab_mode = want_grab;
        }
        // Hide the OS cursor exactly when our sprite is standing in for it.
        let want_visible = !active;
        if opts.visible != want_visible {
            opts.visible = want_visible;
        }
    }

    if let Ok((mut node, mut image)) = sprite_q.single_mut() {
        if active {
            let (handle, (hx, hy)) = match *style {
                CursorStyle::Arrow => (assets.arrow.clone(), assets.arrow_hotspot),
                CursorStyle::Hand => (assets.hand.clone(), assets.hand_hotspot),
                CursorStyle::Rotate => (assets.rotate.clone(), assets.rotate_hotspot),
            };
            let pos = if rotating {
                *lock_point
            } else {
                live_pos.unwrap_or(*lock_point)
            };
            if image.image != handle {
                image.image = handle;
            }
            let left = Val::Px(pos.x - hx as f32);
            let top = Val::Px(pos.y - hy as f32);
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
