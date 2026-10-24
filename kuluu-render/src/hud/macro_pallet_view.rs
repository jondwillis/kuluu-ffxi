//! The macro palette: one book's twenty keytops drawn as the retail two-rows-of-ten band, and the
//! only thing on screen while it is open.
//!
//! `mcr1pall`/`mcr2pall` each record a 480-wide frame carrying ten blocks in one row (read them with
//! `cargo run -p ffxi-dat --example dat-menu-window-print -- mcr1pall mcr2pall`), and the client
//! stacks those two windows to make its grid. Both records sit at x=16 y=48, so the second row hangs
//! below the first rather than coming from a different seat in the DATs. The record's own blocks are
//! 40x32 because they were measured against retail's menu face; kuluu sets its menu type far larger
//! (14 px rows), and a band kept at the raw block sizes reads as shrunken type beside it (owner
//! 2026-10-24: "widen the spce, retail is much widner"). So only two things are taken from the record:
//! where the band sits, and that each row holds ten keytops. The blocks themselves are sized from this
//! file's type metrics instead, which for our monospace HUD font is exact rather than estimated.
//!
//! The owner ruled which keyset draws on top (2026-10-24: "macro menu.. should 2 row horizonel, , alt
//! on top, ctrl on bot"); the records list their two grid windows the other way about, so this stacking
//! follows that ruling rather than record order. Nothing else stands beside a palette (see
//! [`crate::hud::menu::focus_excludes_ancestors`]), which is what his capture of the band shows: books,
//! options and Commands are all gone while it is open.

use bevy::prelude::*;

use crate::hud::client_user_files::{ENTRY_NAME_BYTES, MACRO_BOOK_SLOTS};
use crate::hud::menu::{ConfigDatRows, DynamicMenu, MACRO_PALLET_SLOTS_PER_KEYSET};
use crate::input_mode::{InputMode, MenuKind};
use crate::ui_font;

/// The palette's own DAT record. `mcr2pall` records the identical grid, so either yields the seat.
const PALLET_WINDOW_NAME: &str = "mcr1pall";

/// Pinned seat for a host whose DATs have not been read yet, transcribed from that record (x 16 y 48)
/// so an early-opened palette keeps the retail corner instead of drifting to a default one.
const PINNED_FRAME_X: f32 = 16.0;
const PINNED_FRAME_Y: f32 = 48.0;

/// A tile's display name at kuluu's menu type, and the quieter/smaller hotkey word under it as the
/// retail block prints it.
const TILE_NAME_PX: f32 = 12.0;
const TILE_HOTKEY_PX: f32 = 9.0;
/// Tiles are wide enough for the longest display name a book can hold: the client stores that name in a
/// ten-byte field (see [`crate::hud::client_user_files`]), so nine characters and nothing more. Wider
/// than that, the tile truncates ([`truncate_to_tile`]) rather than pushing into its neighbour (owner
/// 2026-10-24: "cut off any names that dont fit").
const STORED_NAME_MAX_CHARS: usize = ENTRY_NAME_BYTES - 1;
/// Air between a name and the block's edge, and between neighbouring blocks. The record's blocks stand
/// 8 apart in their own scale; one gutter here keeps that proportion at ours.
const TILE_PAD_PX: f32 = 4.0;
const TILE_GUTTER_PX: f32 = 8.0;

#[derive(Component)]
pub struct MacroPalletPanel;

#[derive(Component)]
pub(crate) struct MacroPalletBackdrop;

/// The owner's own marker standing left of the keytop under the cursor, as retail's band prints it.
#[derive(Component)]
pub(crate) struct MacroPalletCursorMark;

/// One keytop tile at its position in the band (alt row first, then ctrl row).
#[derive(Component, Clone, Copy)]
pub(crate) struct MacroPalletTile {
    slot: usize,
}

#[derive(Component, Clone, Copy)]
pub(crate) enum TileTextPart {
    Name(usize),
    Hotkey(usize),
}

/// The band and its twenty tiles. Tiles are pooled at spawn and only re-filled afterwards, so an open
/// palette costs no allocations per frame.
pub(crate) fn spawn_macro_pallet_view(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MacroPalletPanel,
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                ..default()
            },
            ZIndex(crate::hud::style::WINDOW_Z),
        ))
        .with_children(|root| {
            root.spawn((
                MacroPalletBackdrop,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(crate::hud::style::theme::FRAME_BG),
                BorderColor::all(crate::hud::style::theme::FRAME_EDGE),
            ));
            root.spawn((
                MacroPalletCursorMark,
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                Text::new(""),
                crate::hud::style::text_font(TILE_NAME_PX),
                TextColor(crate::hud::style::theme::CURSOR),
            ));
            for slot in 0..MACRO_BOOK_SLOTS {
                root.spawn((
                    MacroPalletTile { slot },
                    Node {
                        position_type: PositionType::Absolute,
                        flex_direction: FlexDirection::Column,
                        justify_content: JustifyContent::SpaceBetween,
                        border: UiRect::all(Val::Px(1.0)),
                        padding: UiRect::axes(Val::Px(TILE_PAD_PX), Val::Px(TILE_PAD_PX / 2.0)),
                        ..default()
                    },
                    BackgroundColor(crate::hud::style::theme::CELL_BG),
                    BorderColor::all(crate::hud::style::theme::CELL_EDGE),
                ))
                .with_children(|tile| {
                    tile.spawn((
                        TileTextPart::Name(slot),
                        Text::new(""),
                        crate::hud::style::text_font(TILE_NAME_PX),
                        TextColor(crate::hud::style::theme::TEXT),
                        Node {
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        TextLayout {
                            justify: Justify::Left,
                            linebreak: LineBreak::NoWrap,
                        },
                    ));
                    tile.spawn((
                        TileTextPart::Hotkey(slot),
                        Text::new(""),
                        crate::hud::style::text_font(TILE_HOTKEY_PX),
                        TextColor(crate::hud::style::theme::MUTED),
                        Node {
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        TextLayout {
                            justify: Justify::Right,
                            linebreak: LineBreak::NoWrap,
                        },
                    ));
                });
            }
        });
}

/// Cut `name` down to what fits in `max_width_px` at `font_px`, marking the cut with three dots. The
/// HUD font is monospace, so this is exact: nothing else in kuluu needs a guess either.
pub(crate) fn truncate_to_tile(name: &str, font_px: f32, max_width_px: f32) -> String {
    let ellipsis = "...";
    if ui_font::text_width_px(name, font_px) <= max_width_px {
        return name.to_string();
    }
    let char_px = ui_font::text_width_px("0", font_px).max(f32::EPSILON);
    let room_chars = (max_width_px / char_px).floor().max(0.0) as usize;
    let keep = room_chars.saturating_sub(ellipsis.chars().count());
    let mut cut: String = name.chars().take(keep).collect();
    cut.push_str(ellipsis);
    cut
}

/// One row of the band is one line of names over one line of hotkey words, plus their padding.
fn tile_size_px() -> (f32, f32) {
    let width = ui_font::text_width_px(
        &"0".repeat(STORED_NAME_MAX_CHARS),
        TILE_NAME_PX.max(TILE_HOTKEY_PX),
    ) + 2.0 * TILE_PAD_PX;
    let height = ui_font::line_height_px(TILE_NAME_PX)
        + ui_font::line_height_px(TILE_HOTKEY_PX)
        + 2.0 * TILE_PAD_PX;
    (width, height)
}

/// Where the band's tiles sit, in the panel's own box: ten columns per row stepping by a tile plus its
/// gutter, rows stepping by a row's height plus the same gutter. Row two is the second keyset.
fn tile_rects() -> Vec<(f32, f32, f32, f32)> {
    let (tile_w, tile_h) = tile_size_px();
    let column_pitch = tile_w + TILE_GUTTER_PX;
    let row_pitch = tile_h + TILE_GUTTER_PX;
    let columns = MACRO_PALLET_SLOTS_PER_KEYSET.max(1);
    let mut rects = Vec::with_capacity(MACRO_BOOK_SLOTS);
    for index in 0..MACRO_BOOK_SLOTS {
        let row = index / columns;
        let column = index % columns;
        rects.push((
            column_pitch * column as f32,
            row_pitch * row as f32,
            tile_w,
            tile_h,
        ));
    }
    rects
}

/// The hotkey word a tile prints, in band order: alt row (1..9 then 0), then ctrl row.
fn hotkey_word(slot: usize) -> &'static str {
    let per_row = crate::hud::menu::MACRO_PALLET_SLOTS_PER_KEYSET;
    match slot / per_row.max(1) {
        0 => crate::hud::menu::MACRO_PALLET_HOTKEYS_ALT[slot % per_row],
        _ => crate::hud::menu::MACRO_PALLET_HOTKEYS[slot % per_row],
    }
}

/// Draw the palette for the focused book, and hide it whenever anything else holds focus. Tile names
/// are the display names read out of the client's own macro files (see [`crate::hud::client_user_files`]),
/// so each block says what the player named that macro; a keytop they left empty stays an empty block,
/// exactly as the retail band leaves it.
pub(crate) fn update_macro_pallet_view(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    config_rows: Res<ConfigDatRows>,
    // Four parameters reach for `&mut Node` and two for `&mut Text`, so every one of them names the
    // others' markers in a `Without`: Bevy only checks isolation when a system first runs, and an
    // unprefixed `With<A>` against `With<B>` is not provably disjoint (it panicked on first frame with
    // error[B0001] over exactly these queries).
    mut panel_q: Query<
        &mut Node,
        (
            With<MacroPalletPanel>,
            Without<MacroPalletBackdrop>,
            Without<MacroPalletCursorMark>,
            Without<MacroPalletTile>,
        ),
    >,
    mut backdrop_q: Query<
        &mut Node,
        (
            With<MacroPalletBackdrop>,
            Without<MacroPalletPanel>,
            Without<MacroPalletCursorMark>,
            Without<MacroPalletTile>,
        ),
    >,
    mut mark_q: Query<
        (&mut Text, &mut Node),
        (
            With<MacroPalletCursorMark>,
            Without<MacroPalletPanel>,
            Without<MacroPalletBackdrop>,
            Without<MacroPalletTile>,
            Without<TileTextPart>,
        ),
    >,
    #[allow(clippy::type_complexity)] mut tile_q: Query<
        (
            &MacroPalletTile,
            &mut Node,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        (
            With<MacroPalletTile>,
            Without<MacroPalletPanel>,
            Without<MacroPalletBackdrop>,
            Without<MacroPalletCursorMark>,
        ),
    >,
    mut text_q: Query<
        (&TileTextPart, &mut Text, &mut TextColor),
        (With<TileTextPart>, Without<MacroPalletCursorMark>),
    >,
) {
    // The band belongs to the focused palette level of the menu stack, and to nothing else: any other
    // mode (or a stack with something else on top) hides it.
    let focused = match &*mode {
        InputMode::Menu(stack) => stack
            .levels
            .last()
            .filter(|level| matches!(level.kind, MenuKind::MacroPallet { .. }))
            .map(|level| level.cursor),
        _ => None,
    };

    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };
    let Some(cursor) = focused else {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    };
    // The panel stands at its own record seat: DAT corner where it has been read, pinned otherwise.
    let (frame_x, frame_y) = config_rows
        .record_frame(PALLET_WINDOW_NAME)
        .map(|frame| (frame.x, frame.y))
        .unwrap_or((PINNED_FRAME_X, PINNED_FRAME_Y));
    if panel.left != Val::Px(frame_x) {
        panel.left = Val::Px(frame_x);
    }
    if panel.top != Val::Px(frame_y) {
        panel.top = Val::Px(frame_y);
    }
    if panel.display != Display::Flex {
        panel.display = Display::Flex;
    }

    let rects = tile_rects();
    let name_room = {
        let (_, _, tile_w, _) = rects.first().copied().unwrap_or((0.0, 0.0, 0.0, 0.0));
        tile_w - 2.0 * TILE_PAD_PX
    };
    if let Ok(mut backdrop) = backdrop_q.single_mut() {
        let right = rects.iter().fold(0.0f32, |acc, r| acc.max(r.0 + r.2));
        let bottom = rects.iter().fold(0.0f32, |acc, r| acc.max(r.1 + r.3));
        backdrop.width = Val::Px(right + TILE_PAD_PX / 2.0);
        backdrop.height = Val::Px(bottom + TILE_PAD_PX / 2.0);
    }

    for (tile, mut node, mut fill, mut edge) in tile_q.iter_mut() {
        let Some(&(x, y, width, height)) = rects.get(tile.slot) else {
            continue;
        };
        node.left = Val::Px(x);
        node.top = Val::Px(y);
        node.width = Val::Px(width);
        node.height = Val::Px(height);
        let on_cursor = tile.slot == cursor;
        fill.0 = if on_cursor {
            crate::hud::style::theme::CURSOR_BG
        } else {
            crate::hud::style::theme::CELL_BG
        };
        let want_edge = if on_cursor {
            crate::hud::style::theme::CURSOR
        } else {
            crate::hud::style::theme::CELL_EDGE
        };
        if edge.left != want_edge {
            *edge = BorderColor::all(want_edge);
        }
    }

    // The cursor's own marker stands in the gutter just left of its block, on that block's name line.
    if let Ok((mut mark, mut node)) = mark_q.single_mut() {
        let (x, y) = rects
            .get(cursor)
            .map(|r| (r.0, r.1 + TILE_PAD_PX / 2.0))
            .unwrap_or((0.0, 0.0));
        node.left = Val::Px(x - ui_font::text_width_px(">", TILE_NAME_PX) - 2.0);
        node.top = Val::Px(y);
        if **mark != ">" {
            **mark = ">".to_string();
        }
    }

    for (part, mut text, mut color) in text_q.iter_mut() {
        match *part {
            TileTextPart::Name(slot) => {
                let name = dynamic
                    .rows
                    .get(slot)
                    .map(|row| row.label.as_str())
                    .unwrap_or_default();
                let shown = truncate_to_tile(name, TILE_NAME_PX, name_room);
                if **text != shown.as_str() {
                    **text = shown;
                }
                color.0 = if slot == cursor {
                    crate::hud::style::theme::CURSOR
                } else if name.is_empty() {
                    crate::hud::style::theme::FAINT
                } else {
                    crate::hud::style::theme::TEXT
                };
            }
            TileTextPart::Hotkey(slot) => {
                let word = hotkey_word(slot);
                if **text != word {
                    **text = word.to_string();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bevy checks a system's parameter isolation the first time it runs, not at compile time, so this
    /// surface (four `&mut Node` and two `&mut Text` parameters over one spawned tree) would ship broken
    /// without a pass that actually schedules it.
    #[test]
    fn the_band_survives_a_frame_of_its_own_systems() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputMode>()
            .init_resource::<DynamicMenu>()
            .init_resource::<ConfigDatRows>()
            .add_systems(Startup, spawn_macro_pallet_view)
            .add_systems(Update, update_macro_pallet_view);
        app.update();

        // Spawned alone, the band waits for a focused palette: nothing shows.
        let mut panels = app
            .world_mut()
            .query_filtered::<&Node, With<MacroPalletPanel>>();
        assert_eq!(
            panels.single(app.world()).expect("one panel").display,
            Display::None
        );
    }

    #[test]
    fn a_tile_is_sized_for_the_longest_name_a_book_can_hold() {
        let (tile_w, tile_h) = tile_size_px();
        assert!(
            ui_font::text_width_px(&"0".repeat(STORED_NAME_MAX_CHARS), TILE_NAME_PX)
                <= tile_w - 2.0 * TILE_PAD_PX,
            "the widest storable display name has to fit inside a tile"
        );
        assert!(tile_h > ui_font::line_height_px(TILE_NAME_PX));

        let rects = tile_rects();
        assert_eq!(rects.len(), MACRO_BOOK_SLOTS);
        // Ten columns per row, and the band's second row starts under its first.
        assert_eq!(rects[MACRO_PALLET_SLOTS_PER_KEYSET - 1].0, {
            let (w, _) = tile_size_px();
            (w + TILE_GUTTER_PX) * (MACRO_PALLET_SLOTS_PER_KEYSET - 1) as f32
        });
        assert!(rects[MACRO_PALLET_SLOTS_PER_KEYSET].1 > rects[0].1);
    }

    #[test]
    fn a_name_wider_than_its_tile_is_cut_and_marked() {
        let (tile_w, _) = tile_size_px();
        let room = tile_w - 2.0 * TILE_PAD_PX;
        assert_eq!(truncate_to_tile("InviteSW", TILE_NAME_PX, room), "InviteSW");
        let cut = truncate_to_tile("UnreasonablyLongName", TILE_NAME_PX, room);
        assert!(cut.ends_with("..."));
        assert!(
            ui_font::text_width_px(&cut, TILE_NAME_PX) <= room + 1.0,
            "a cut name has to sit inside the tile it was cut for"
        );
    }

    #[test]
    fn hotkey_words_follow_the_band_order() {
        assert_eq!(hotkey_word(0), "Alt1");
        assert_eq!(hotkey_word(9), "Alt0");
        assert_eq!(hotkey_word(10), "Ctrl1");
        assert_eq!(hotkey_word(MACRO_BOOK_SLOTS - 1), "Ctrl0");
    }
}
