//! Menu window records inside the UI DATs — the §16 mini-chunk law and the
//! §43 `windowpsNNN` caption law from `vault main_menu.md`, turned into a reader.
//!
//! A DAT of this family is a chain of chunks; each chunk carries one window:
//! header `[fourcc 4][u32 kind+size]` (`kind = v & 0x7F`,
//! `size = ((v >> 7) & 0xFFFFF) * 16`; the law shared with the vault N-pass
//! scanner), and the window's own record name `menu    <root>` sits at chunk
//! offset +0x10. The records following it are variable-length, so children are
//! located by their names inside the chunk body: `menu    windowps` followed
//! by a **digit-run caption id ended by NUL** — up to five ASCII digits
//! (`windowps165`, `windowps1092`; whose value is the caption id into catalog
//! table 11 = ROM/165/75.DAT, the §43 law read from the conf-class GetText
//! sweep, FFXiMain.dll retail-2026-09 RVA 0x196ED1 `push 0x8f; push 0xb`). A
//! suffix that is not ASCII digits (a small binary id) marks a chrome/anchor
//! entry, which §43 excludes from row tables. Repeated caption ids inside one
//! chunk are separate records — retail pages genuinely show the same option
//! twice (e.g. conf7 shows 219 and 222 twice each; measured in vault §57).
//!
//! No DAT generation is ever hand-picked: [`windows_from_root`] consults the
//! candidate UI DAT files newest-first and matches windows by **record root**,
//! so looking up `conf6win` can only resolve in the named-root generation
//! (vault main_menu.md §49) while short-root-only windows such as `conf10`
//! fall back to the legacy base DAT. Each candidate id resolves through the
//! install's own VTABLE/FTABLE via [`DatRoot::resolve`] — the client's
//! mechanism, per the owner's no-stale-reads rule.

use std::collections::BTreeMap;

use crate::archive::DatRoot;
use crate::ui_element::UI_SHEET_FILE_ID;
use crate::{DatError, Result};

/// The legacy base UI DAT (`ROM/0/1.DAT` on every measured install) — the
/// short-root generation of vault main_menu.md §47. Last candidate: windows
/// whose roots exist only there (e.g. `conf10`) still resolve.
pub const LEGACY_UI_DAT_FILE_ID: u32 = 1;

/// The named-root window DAT — same file as [`UI_SHEET_FILE_ID`]
/// (`ROM/119/51.DAT` on horizonxi-2023 and retail-2026-09, whose
/// config-family chunks vault main_menu.md §49 pins). First candidate: the
/// newer generation wins for any root it carries.
pub const NAMED_ROOT_UI_DAT_FILE_ID: u32 = UI_SHEET_FILE_ID;

/// GetText table 0x0E (`ROM/165/75.DAT`, EN, `parse_d_msg` n=1099 — the GetText
/// table behind every `windowpsNNN` caption, vault main_menu.md §43). Menu
/// item `textA` ids resolve here too (selector help lines; content-verified
/// row by row on retail-2026-09 against the handler semantics of FFXiMain.dll
/// retail-2026-09 RVA 0x148D00, vault main_menu.md §59).
pub const CAPTION_CATALOG_TABLE_14_FILE_ID: u32 = 55651;

/// The short-caption catalog (`ROM/165/76.DAT`, EN, `parse_d_msg` n=201 —
/// menu item `textB` ids decode exactly the Command-menu row labels there,
/// e.g. `Status` (81), `Magic List` (76) and `Set Bazaar` (15); file id
/// resolved through the install's own FTABLE word law, vault main_menu.md
/// §59). GetText wrapper: FFXiMain.dll retail-2026-09 RVA 0x152EF0.
pub const CAPTION_SHORT_CATALOG_FILE_ID: u32 = 55652;

/// GetText **table 12** (`parse_d_msg` n=526 — the runtime caption source the roomlist
/// remodel driver fills its five dynamic slots from, .text FFXiMain.dll retail-2026-09
/// 0x1D6900 pushing (12, 450/446..449), plus the confirm prompt (12, 451); vault
/// main_menu.md §59 addendum 21). File id resolved through the install's own FTABLE.
pub const GETTEXT_TABLE_12_CATALOG_FILE_ID: u32 = 55653;

/// GetText **table 9** (`parse_d_msg` n=348 — the chat-message catalog behind the client's
/// local emitter `.text FFXiMain.dll retail-2026-09 RVA 0x129DE0`, vault main_menu.md §59
/// addendum 35). Ids **0x12D..0x134** are `/furcc`'s and `/layoutctrl`'s confirmation and
/// state-query lines (bead kuluu-o9qt7.41); idx 0xC3 = `Q&A` is §34's pairing pin against the
/// JP twin ROM/165/56.DAT. File id resolved through the install's own FTABLE word law
/// (`real_table9_chat_message_catalog_carries_the_option_toggle_lines`).
pub const GETTEXT_TABLE_9_CATALOG_FILE_ID: u32 = 55648;

/// GetText **table 7**, the client-message catalog the alter ego point and merit refusals print
/// from: a d_msg file on the same English language row as table 9
/// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md, "String
/// and graphic sources").
pub const GETTEXT_TABLE_7_CATALOG_FILE_ID: u32 = 55646;

/// The myroom door row's caption pair, fetched by the window's update slot as
/// GetText **table 0x0E** — closed when the door byte is zero (FFXiMain.dll retail-2026-09
/// `.text 0x145439`) and open otherwise (`.text 0x145357`). Table 0x0E is
/// [`CAPTION_CATALOG_TABLE_14_FILE_ID`]; the row's own DAT caption token stays -1.
pub const MOGHOUSE_DOOR_CAPTION_CLOSED_ID: usize = 666;
pub const MOGHOUSE_DOOR_CAPTION_OPEN_ID: usize = 667;
/// …and its help-line pair, GetText **table 0x0D** = [`CAPTION_SHORT_CATALOG_FILE_ID`], closed
/// FFXiMain.dll retail-2026-09 `.text 0x14541F` / open `.text 0x14533D`. The two setters write
/// different widget fields (caption `+0x4C` via `.text 0x11ADB0`, help `+0x50` via `.text 0x11ADE0`).
pub const MOGHOUSE_DOOR_HELP_CLOSED_ID: usize = 147;
pub const MOGHOUSE_DOOR_HELP_OPEN_ID: usize = 148;

/// Candidate list [`windows_from_root`] consults, newest first.
pub const MENU_WINDOW_DAT_FILE_IDS: &[u32] = &[NAMED_ROOT_UI_DAT_FILE_ID, LEGACY_UI_DAT_FILE_ID];

const WINDOW_PREFIX: &[u8] = b"menu    ";
const WIDGET_NAME: &[u8] = b"menu    windowps";

/// §43 suffix law bound: caption ids are at most five ASCII digits before the
/// NUL (highest measured on retail-2026-09: 1096, vault main_menu.md §57).
const MAX_CAPTION_DIGITS: usize = 5;

/// One child widget located inside a window's chunk. `caption_id` is set for
/// the ASCII-digit `windowpsNNN` widgets (catalog table-11 caption id, §43);
/// binary-id chrome entries leave it `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowWidget {
    pub offset: usize,
    pub caption_id: Option<u32>,
}

/// The window's frame block — its own position in the 512x448 menu design
/// space. Field law from the record loaders FFXiMain.dll retail-2026-09
/// RVA 0x11B3B0 (block at record +0x20: `len, x, y, cursor a, cursor b, w, h`
/// as s16s; anchor u8 at block +0x13), placement arithmetic at RVA 0x1193F0:
/// anchor bit 1 adds `W - 512` (right edge) and bit 2 adds `H - 448` (bottom),
/// where W/H are the live menu-layer size. Companion windows own their frame;
/// nothing in a DAT marks one as "beside" another — the DLL opens them with
/// the selector left open (OpenMenu RVA 0x15E350, `activate=1, exclusive=0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameBlock {
    pub x: i16,
    pub y: i16,
    pub cursor_a: i16,
    pub cursor_b: i16,
    pub width: i16,
    pub height: i16,
    /// 0 none, 1 right-edge, 2 bottom-edge, 3 both.
    pub anchor: u8,
}

/// One selectable item block (record loaders FFXiMain.dll retail-2026-09
/// RVA 0x11B6B0). `id` (block +0x12) is what the window handler receives and
/// what selector tables dispatch on; the neighbor bytes (+0x17 Up, +0x18 Down,
/// +0x19 Right, +0x1A Left) are per-item s8 target ids — negative means no
/// move — so arrow navigation is data, never an axis rule (vault §5 nav law:
/// L/R can never break Up/Down when rows follow these bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemBlock {
    pub id: i16,
    /// x/y relative to the window origin.
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
    /// Neighbor item ids; `None` when the byte is negative (blocked).
    pub up: Option<i16>,
    pub down: Option<i16>,
    pub right: Option<i16>,
    pub left: Option<i16>,
    /// The item's two UI-string ids, stored as ASCII decimals inside the
    /// block (loader FFXiMain.dll retail-2026-09 RVA 0x11B6B0; field table of
    /// vault brief §7). `text_a_id` indexes [`CAPTION_CATALOG_TABLE_14_FILE_ID`]
    /// (descriptions — the only text statcom2 carries), `text_b_id` indexes
    /// [`CAPTION_SHORT_CATALOG_FILE_ID`] (short row captions; menuwind/socialme
    /// carry these, statcom2 stores -1). `-1` or empty → `None`.
    pub text_a_id: Option<i32>,
    pub text_b_id: Option<i32>,
}

fn read_i16(bytes: &[u8], at: usize) -> Option<i16> {
    bytes
        .get(at..at + 2)
        .map(|w| i16::from_le_bytes([w[0], w[1]]))
}

/// One window record (one mini-chunk) and its children in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuWindow {
    /// Chunk-header fourcc naming this window's block (`con7` for `conf3win`;
    /// §16 warns fourccs are not always name prefixes).
    pub fourcc: String,
    /// The record root — the exact name FFXiMain.dll's menu manager opens.
    pub root: String,
    pub offset: usize,
    pub size: usize,
    pub widgets: Vec<WindowWidget>,
    /// Item count declared in the record header byte at +0x11 (record loader
    /// FFXiMain.dll retail-2026-09 RVA 0x11B010).
    pub item_count: u8,
    /// The frame block (position, size, anchor) when present.
    pub frame: Option<FrameBlock>,
    /// Item blocks walked by their length words right after the frame block.
    /// `None` entries mark a block whose declared span cut a field short.
    pub items: Vec<Option<ItemBlock>>,
}

impl MenuWindow {
    /// The `windowpsNNN` caption ids in file order — the page's row set and
    /// its option-store keys (vault main_menu.md §43/§47). Repeated ids are
    /// repeated rows, kept.
    pub fn option_row_ids(&self) -> Vec<u32> {
        self.widgets.iter().filter_map(|w| w.caption_id).collect()
    }

    /// The window's parsed items ordered as they DISPLAY: sorted by the block's
    /// y (then x) — file order is id order (the loader expects id = 1-based
    /// position, FFXiMain.dll retail-2026-09 RVA 0x11B6B0), but retail draws
    /// rows wherever their y says. Measured on statcom2/menuwind: the Synthesis
    /// row (menuwind id 10) and Master Levels (statcom2 id 12) both display out
    /// of id order purely from their block y.
    pub fn items_in_display_order(&self) -> Vec<&ItemBlock> {
        let mut items: Vec<&ItemBlock> = self.items.iter().flatten().collect();
        items.sort_by_key(|it| (it.y, it.x));
        items
    }

    /// The window's selectable rows as retail defines them: item blocks in
    /// FILE order (= select id − 1, §7), dropping chrome — blocks carrying no
    /// UI string at all (§59). Config windows prove this is the row law and
    /// NOT the `windowpsNNN` widget list: `conf5win` declares 8 widgets but
    /// draws and dispatches its 16 captioned items (vault main_menu.md §43/
    /// §57; engine bead kuluu-o9qt7.20.1), and the Config hub hides its one
    /// chrome block (id 14, no string).
    pub fn live_items(&self) -> Vec<&ItemBlock> {
        self.items
            .iter()
            .flatten()
            .filter(|it| it.text_a_id.is_some() || it.text_b_id.is_some())
            .collect()
    }

    /// Rows as catalog strings; a caption missing from the table falls back to
    /// its id so a row is never silently dropped.
    pub fn option_rows(&self, mut catalog: impl FnMut(usize) -> Option<String>) -> Vec<String> {
        self.option_row_ids()
            .into_iter()
            .map(|id| {
                let text = usize::try_from(id).ok().and_then(&mut catalog);
                text.unwrap_or_else(|| format!("windowps{id}"))
            })
            .collect()
    }
}

/// Chunk headers along the chain law (vault N-pass tool 3): stop at the first
/// header with a short chunk or one that runs past EOF.
fn chunk_headers(bytes: &[u8]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 16 <= bytes.len() {
        let Ok(name) = std::str::from_utf8(&bytes[off..off + 4]) else {
            break;
        };
        let v = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]);
        // Masked to the DAT's 20-bit size field, so it always fits a usize.
        let size = ((v >> 7) & 0xFFFFF) as usize * 16;
        if size < 16 || off + size > bytes.len() {
            break;
        }
        out.push((name.to_string(), off, size));
        off += size;
    }
    out
}

/// `windowps` suffix law: ASCII digits ended by NUL → caption id; otherwise a
/// small binary id (chrome/anchor, §43) → `None`. The slice runs past the
/// longest possible digit run so 4- and 5-digit ids keep their terminator.
fn parse_widget_suffix(suffix: &[u8]) -> Option<u32> {
    let end = suffix.iter().position(|b| *b == 0)?;
    if end == 0 || end > MAX_CAPTION_DIGITS || !suffix[..end].iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(&suffix[..end]).ok()?.parse().ok()
}

/// Parse every window record in one UI DAT file's bytes.
pub fn parse_windows(bytes: &[u8]) -> Vec<MenuWindow> {
    let mut out = Vec::new();
    for (fourcc, off, size) in chunk_headers(bytes) {
        // The window's own record name sits at +0x10 (§16 mini-chunk shape).
        let Some(head) = bytes.get(off + 0x10..off + 0x20) else {
            continue;
        };
        if &head[..WINDOW_PREFIX.len()] != WINDOW_PREFIX {
            continue;
        }
        // The manager's 16-byte name field keeps `menu    ` on the front and
        // space-pads (or NUL-terminates) the short root behind it.
        let rest = &head[WINDOW_PREFIX.len()..];
        let padded = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        let Ok(root_full) = std::str::from_utf8(&rest[..padded]) else {
            continue;
        };
        let root = root_full.trim_end().to_string();
        if !root.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
            continue;
        }
        // Frame + item blocks (§7 law): back-to-back length-worded blocks
        // starting at record +0x20 (= chunk +0x30), walked exactly as the
        // loaders FFXiMain.dll retail-2026-09 RVA 0x11B010 walk them.
        let declared_items = bytes.get(off + 0x21).copied().unwrap_or(0);
        let (frame, items) = parse_blocks(bytes, off + 0x30, off + size, declared_items);

        // Children: widget names inside the body, located by scan (records are
        // variable-length and not quad-aligned in a stable way).
        let body_start = off + 0x20;
        let body_end = off + size;
        let mut widgets = Vec::new();
        let mut from = body_start;
        while from < body_end {
            let Some(rel) = find_in_range(bytes, body_end, WIDGET_NAME, from) else {
                break;
            };
            let after_name = rel + WIDGET_NAME.len();
            // The NUL of a digit id can sit up to MAX_CAPTION_DIGITS bytes in.
            let look_end = (after_name + 1 + MAX_CAPTION_DIGITS).min(bytes.len());
            widgets.push(WindowWidget {
                offset: rel,
                caption_id: parse_widget_suffix(&bytes[after_name..look_end]),
            });
            from = after_name;
        }
        out.push(MenuWindow {
            fourcc,
            root,
            offset: off,
            size,
            widgets,
            item_count: declared_items,
            frame,
            items,
        });
    }
    out
}

/// Walk the back-to-back blocks after a window's header. The first block is
/// the frame; the following `declared` are items (FFXiMain.dll retail-2026-09
/// RVA 0x11B3B0/0x11B6B0). A length word outside `MIN_BLOCK_LEN..=remaining`
/// ends the walk — the loaders stop there too. Blocks too short for their
/// fields still consume their span and yield `None` items.
fn parse_blocks(
    bytes: &[u8],
    mut at: usize,
    end: usize,
    declared_items: u8,
) -> (Option<FrameBlock>, Vec<Option<ItemBlock>>) {
    let block_len = |at: usize| -> Option<usize> {
        let raw = read_i16(bytes, at)?;
        if !(MIN_BLOCK_LEN..=0x2000).contains(&raw) || at + raw as usize > end {
            return None;
        }
        Some(raw as usize)
    };
    let mut frame = None;
    if let Some(len) = block_len(at) {
        let b = at;
        // Frame fields (FFXiMain.dll retail-2026-09 RVA 0x11B3B0).
        if len >= FRAME_BLOCK_MIN_LEN as usize {
            let (Some(x), Some(y)) = (read_i16(bytes, b + OFF_X), read_i16(bytes, b + OFF_Y))
            else {
                return (None, Vec::new());
            };
            frame = Some(FrameBlock {
                x,
                y,
                cursor_a: read_i16(bytes, b + OFF_CURSOR_A).unwrap_or(0),
                cursor_b: read_i16(bytes, b + OFF_CURSOR_B).unwrap_or(0),
                width: read_i16(bytes, b + OFF_WIDTH).unwrap_or(0),
                height: read_i16(bytes, b + OFF_HEIGHT).unwrap_or(0),
                anchor: bytes[b + OFF_ANCHOR],
            });
        }
        at += len;
    } else {
        return (None, Vec::new());
    }
    let mut items = Vec::new();
    while items.len() < declared_items as usize {
        let Some(len) = block_len(at) else { break };
        let b = at;
        // Item fields (FFXiMain.dll retail-2026-09 RVA 0x11B6B0).
        let item = if len >= ITEM_BLOCK_MIN_LEN as usize {
            // Negative s8 means "no move in this direction" (blocked).
            let nb = |off: usize| match bytes[b + off] {
                v if (v as i8) < 0 => None,
                v => Some(v as i16),
            };
            let (text_a_id, text_b_id) = read_block_text(bytes, b, ITEM_TEXT_A_PART_COUNT_OFF);
            Some(ItemBlock {
                id: read_i16(bytes, b + OFF_ITEM_ID).unwrap_or(0),
                x: read_i16(bytes, b + OFF_X).unwrap_or(0),
                y: read_i16(bytes, b + OFF_Y).unwrap_or(0),
                width: read_i16(bytes, b + OFF_WIDTH).unwrap_or(0),
                height: read_i16(bytes, b + OFF_HEIGHT).unwrap_or(0),
                up: nb(OFF_NEIGHBORS),
                down: nb(OFF_NEIGHBORS + 1),
                right: nb(OFF_NEIGHBORS + 2),
                left: nb(OFF_NEIGHBORS + 3),
                text_a_id,
                text_b_id,
            })
        } else {
            None
        };
        items.push(item);
        at += len;
    }
    (frame, items)
}

/// Smallest block a length word may claim (smallest measured item/frame
/// block is 0x40 on retail-2026-09; `ITEM_BLOCK_MIN_LEN` is the field floor,
/// so accept from there and let the loader's own stop rule — overrun — end
/// short chains).
const MIN_BLOCK_LEN: i16 = ITEM_BLOCK_MIN_LEN;

/// Block field offsets, counted from the block start (FFXiMain.dll
/// retail-2026-09 record loaders RVA 0x11B3B0/0x11B6B0).
const OFF_X: usize = 0x02;
const OFF_Y: usize = 0x04;
const OFF_CURSOR_A: usize = 0x06;
const OFF_CURSOR_B: usize = 0x08;
const OFF_WIDTH: usize = 0x0A;
const OFF_HEIGHT: usize = 0x0C;
/// Frame anchor byte, u8 (vault brief §4 placement law).
const OFF_ANCHOR: usize = 0x13;
/// Item handler id, s16 (vault brief §7).
const OFF_ITEM_ID: usize = 0x12;
/// First of the four neighbor s8s: Up, Down, Right, Left.
const OFF_NEIGHBORS: usize = 0x17;
/// Item block: part count byte (then textA/textB length bytes at +2/+3),
/// parts from 0x20 followed by the two ASCII-decimal string ids (loader
/// FFXiMain.dll retail-2026-09 RVA 0x11B6B0, vault brief §7).
const ITEM_TEXT_A_PART_COUNT_OFF: usize = 0x1B;
const ITEM_BLOCKS_BODY_OFF: usize = 0x20;

/// Read an item block's two text fields: after `parts * 20` bytes from +0x20
/// sit textA (decimal digits, NUL) then textB. A `-1`, empty or non-digit run
/// reads as `None` — the record says "no string here".
fn read_block_text(
    bytes: &[u8],
    block: usize,
    part_count_off: usize,
) -> (Option<i32>, Option<i32>) {
    let Some(len_a_raw) = bytes.get(block + part_count_off + 2).copied() else {
        return (None, None);
    };
    let parts = bytes.get(block + part_count_off).copied().unwrap_or(0) as usize;
    let text_start = block + ITEM_BLOCKS_BODY_OFF + parts * 20;
    let parse_at = |at: Option<usize>, len: u8| -> Option<i32> {
        let at = at?;
        if len == 0 {
            return None;
        }
        let run = std::str::from_utf8(bytes.get(at..at + len as usize)?).ok()?;
        run.trim().parse::<i32>().ok().filter(|v| *v >= 0)
    };
    let a = parse_at(Some(text_start), len_a_raw);
    let b_at = text_start.checked_add(len_a_raw as usize + 1);
    let len_b = bytes.get(block + part_count_off + 3).copied().unwrap_or(0);
    let b = parse_at(b_at, len_b);
    (a, b)
}
/// A frame block must at least cover through the anchor byte.
const FRAME_BLOCK_MIN_LEN: i16 = (OFF_ANCHOR + 1) as i16;
/// An item block must at least cover all four neighbor bytes.
const ITEM_BLOCK_MIN_LEN: i16 = (OFF_NEIGHBORS + 4) as i16;

fn find_in_range(hay: &[u8], end: usize, needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || end > hay.len() {
        return None;
    }
    let last_start = end - needle.len();
    (from..=last_start).find(|i| &hay[*i..*i + needle.len()] == needle)
}

/// Blocks a chunk actually yielded: its frame plus every item block whose fields
/// survived the walk. Zero means the body carried no length-worded chain at all.
fn parsed_block_count(window: &MenuWindow) -> usize {
    usize::from(window.frame.is_some()) + window.items.len()
}

/// Root-keyed accumulator of [`windows_from_root`], split out so the collision
/// rule is testable without an install.
#[derive(Default)]
struct MergedWindows(BTreeMap<String, (u32, MenuWindow)>);

impl MergedWindows {
    fn merge(&mut self, file_id: u32, windows: impl IntoIterator<Item = MenuWindow>) {
        for window in windows {
            if let Some((prev_file, prev)) = self.0.get(&window.root) {
                // A later chunk from the same file only replaces an earlier one
                // that parsed at least as many blocks; see `windows_from_root`.
                if *prev_file == file_id && parsed_block_count(prev) >= parsed_block_count(&window)
                {
                    continue;
                }
            }
            self.0.insert(window.root.clone(), (file_id, window));
        }
    }

    fn into_windows(self) -> BTreeMap<String, MenuWindow> {
        self.0
            .into_iter()
            .map(|(root, (_file_id, window))| (root, window))
            .collect()
    }
}

/// All windows of the candidate menu DATs keyed by record root; newest file
/// wins per root ([`MENU_WINDOW_DAT_FILE_IDS`] is newest-first).
///
/// Same-file collisions need a second rule. The UI DAT carries, for some roots,
/// a *second* chunk with the same fourcc and name whose body is not the block
/// chain at all — measured on retail-2026-09 `ROM/119/51.DAT`: `cfil`/`cfilter`
/// has its record at 0x2819B0 (frame + 14 items) and an argument-less twin
/// (`MENU`-run body, header word 0x4D7F) later at 0x2AAC30; the same shape repeats for `evit`/`evitem`
/// (0x2A98C0), `buff` (0x2AACB0) and `fep` (0x2A43F0), and identically in the
/// legacy `ROM/0/1.DAT`. Only one of them is what the record loaders
/// (FFXiMain.dll retail-2026-09 RVA 0x11B010 → frame 0x11B3B0, item 0x11B6B0)
/// can walk, so a chunk that yields blocks must never be shadowed by its
/// block-less twin *within one file* [I]. Across files the newest generation
/// still wins outright, which is what §47/§49 pins.
pub fn windows_from_root(dat: &DatRoot) -> Result<BTreeMap<String, MenuWindow>> {
    let mut merged = MergedWindows::default();
    // Walk oldest→newest and overwrite, so the newest file wins for a root.
    for &file_id in MENU_WINDOW_DAT_FILE_IDS.iter().rev() {
        let loc = dat.resolve(file_id)?;
        let path = loc.path_under(dat);
        let bytes = std::fs::read(&path).map_err(|source| DatError::Io { path, source })?;
        merged.merge(file_id, parse_windows(&bytes));
    }
    let map = merged.into_windows();
    if map.is_empty() {
        return Err(DatError::Io {
            path: dat.root().to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no menu window record parsed from any candidate UI DAT",
            ),
        });
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full chunk: header, the §16 zero-pad up to +0x10 (where the window
    /// record name sits), then the body, padded up to the declared size.
    fn chunk(fourcc: &[u8; 4], mut body: Vec<u8>) -> Vec<u8> {
        let size_words = (body.len() + 0x10).div_ceil(16) as u32;
        let mut out = fourcc.to_vec();
        out.extend_from_slice(&(size_words << 7 | 0x30u32).to_le_bytes());
        out.resize(0x10, 0);
        out.append(&mut body);
        let target = size_words as usize * 16;
        out.resize(target.max(out.len()), 0);
        out
    }

    /// Root record: the padded window name at +0x10 plus the §16 geometry word-run.
    fn root_record(root: &str) -> Vec<u8> {
        let mut r = format!("menu    {root}").into_bytes();
        while r.len() % 4 != 0 || r.len() < 0x10 {
            // keep the name field's padded tail quad-clean
            r.push(0);
        }
        r.extend_from_slice(&[0x50, 0x00, 0x10, 0x00, 0x30, 0x00, 0x00, 0x00]);
        r
    }

    /// A widget record as the real chunks pack them: previous-record tail, then
    /// `menu    windowpsNNN`, anchor `-1`, geometry words.
    fn widget(id: &str) -> Vec<u8> {
        let mut r = vec![0x00, 0x03, 0x02, 0x00];
        r.extend_from_slice(format!("menu    windowps{id}\0").as_bytes());
        r.extend_from_slice(b"-1\0");
        r.extend_from_slice(&[0x40, 0x00, 0xAE, 0x00, 0x3F, 0x00, 0x00, 0x00]);
        r
    }

    #[test]
    fn parses_a_synthetic_named_root_window() {
        // Shaped after ROM/119/51.DAT chunk 'con7' @0x28AC00 (vault §49).
        let mut body = root_record("conf3win");
        for id in ["165", "166", "181"] {
            body.extend_from_slice(&widget(id));
        }
        // A binary-id chrome widget (§43): no ASCII, must parse without caption.
        let mut chrome = vec![0x00, 0x03, 0x02, 0x00];
        chrome.extend_from_slice(b"menu    windowps\x06\0");
        body.extend_from_slice(&chrome);

        let bytes = chunk(b"con7", body);
        let windows = parse_windows(&bytes);
        assert_eq!(windows.len(), 1, "one window per mini-chunk");
        let w = &windows[0];
        assert_eq!((w.fourcc.as_str(), w.root.as_str()), ("con7", "conf3win"));
        // Caption ids in file order; the chrome entry contributes none.
        assert_eq!(w.option_row_ids(), vec![165, 166, 181]);

        let rows = w.option_rows(|i| Some(format!("cap{i}")));
        assert_eq!(
            rows,
            vec!["cap165".to_string(), "cap166".into(), "cap181".into()]
        );
    }

    #[test]
    fn multi_digit_ids_keep_their_terminator() {
        // co22 'conf13wi' carries windowps1088 (vault §57): the NUL lands past
        // a naive 4-byte suffix slice.
        let mut body = root_record("conf13wi");
        for id in ["986", "147", "148", "1088", "1090", "1089"] {
            body.extend_from_slice(&widget(id));
        }
        let bytes = chunk(b"co22", body);
        assert_eq!(
            parse_windows(&bytes)[0].option_row_ids(),
            vec![986, 147, 148, 1088, 1090, 1089]
        );
    }

    #[test]
    fn suffix_law_separates_ids_from_binary_chrome() {
        assert_eq!(parse_widget_suffix(b"165\0"), Some(165));
        assert_eq!(parse_widget_suffix(b"985\0"), Some(985));
        assert_eq!(parse_widget_suffix(b"1088\0"), Some(1088));
        assert_eq!(parse_widget_suffix(b"\x06\0\0\0"), None); // chrome, §43
        assert_eq!(parse_widget_suffix(b"\0\0\0\0"), None);
        assert_eq!(parse_widget_suffix(b"12\x00\x00"), Some(12));
        // Six digits is beyond the measured id space — treat as not a caption.
        assert_eq!(parse_widget_suffix(b"123456\0"), None);
    }

    /// Frame + item blocks decoded from bytes shaped after the real chunks
    /// (persona/statcom2, ROM/119/51.DAT): position, anchor and per-item
    /// neighbor ids come out of the length-worded block walk.
    #[test]
    fn decodes_frame_and_item_blocks() {
        // The record body is name (0x10 bytes) then the header padded out to
        // 0x20 — frame and item blocks start at body +0x20 (= chunk +0x30).
        let mut full = root_record("statcom2");
        full.resize(0x20, 0);
        // Item count lives after the name in the header (FFXiMain.dll
        // retail-2026-09 RVA 0x11B010).
        full[0x11] = 2;

        // Frame block (len 0x50): x=384 y=48 w=112 h=218 anchor@+0x13 = 1.
        let mut frame = vec![0x50, 0x00];
        frame.extend_from_slice(&384i16.to_le_bytes());
        frame.extend_from_slice(&48i16.to_le_bytes());
        while frame.len() < OFF_WIDTH {
            frame.push(0);
        }
        frame.extend_from_slice(&112i16.to_le_bytes());
        frame.extend_from_slice(&218i16.to_le_bytes());
        frame.resize(0x50, 0);
        frame[OFF_ANCHOR] = 1;

        // Two item blocks (len 0x40): ids 1 and 2, vertically chained,
        // left/right blocked (-1).
        let item_block = |id: i16, up: u8, down: u8, right: u8, left: u8| {
            let mut v = vec![0x40, 0x00];
            while v.len() < OFF_ITEM_ID {
                v.push(0);
            }
            v.extend_from_slice(&id.to_le_bytes());
            while v.len() < OFF_NEIGHBORS {
                v.push(0);
            }
            v.extend_from_slice(&[up, down, right, left]);
            v.resize(0x40, 0);
            v
        };
        let it1 = item_block(1, 1, 2, 0xFF, 0xFF);
        let it2 = item_block(2, 1, 1, 0xFF, 0xFF);

        let mut chunk_body = full;
        chunk_body.extend_from_slice(&frame);
        chunk_body.extend_from_slice(&it1);
        chunk_body.extend_from_slice(&it2);
        let bytes = chunk(b"sta2", chunk_body);
        let windows = parse_windows(&bytes);
        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        assert_eq!(w.item_count, 2);
        assert_eq!(
            w.frame,
            Some(FrameBlock {
                x: 384,
                y: 48,
                cursor_a: 0,
                cursor_b: 0,
                width: 112,
                height: 218,
                anchor: 1
            })
        );
        let items: Vec<&ItemBlock> = w.items.iter().flatten().collect();
        assert_eq!(items.len(), 2);
        assert_eq!(
            (items[0].id, items[0].up, items[0].down),
            (1, Some(1), Some(2))
        );
        assert_eq!((items[0].right, items[0].left), (None, None));
        assert_eq!(items[1].id, 2);
    }

    /// Gated on a retail install (self-skips without one). Frames measured on
    /// ROM/119/51.DAT via the §7 walk: selectors wear x=384 anchor 1 (right
    /// edge of the design space), companions sit left — conf2win at x=16,
    /// persona at (16,48) with its code-move on top, profile at x=130.
    #[test]
    fn real_windows_carry_their_own_frames() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let windows = windows_from_root(&root).expect("menu windows");
        let frame_of = |name: &str| -> FrameBlock {
            *windows[name]
                .frame
                .as_ref()
                .unwrap_or_else(|| panic!("{name} frame missing"))
        };
        assert_eq!(
            frame_of("persona"),
            FrameBlock {
                x: 16,
                y: 48,
                cursor_a: 0,
                cursor_b: 0,
                width: 112,
                height: 190,
                anchor: 0
            }
        );
        assert_eq!(
            frame_of("profile"),
            FrameBlock {
                x: 130,
                y: 48,
                cursor_a: 0,
                cursor_b: 0,
                width: 252,
                height: 178,
                anchor: 0
            }
        );
        for selector in ["menuwind", "socialme", "configwi", "statcom2"] {
            let f = frame_of(selector);
            assert_eq!((f.x, f.anchor), (384, 1), "{selector} right-edge law");
            assert_eq!(
                (f.width, f.height),
                (
                    112,
                    if selector == "menuwind" || selector == "socialme" {
                        202
                    } else {
                        218
                    }
                ),
                "{selector} size"
            );
        }
        let page = frame_of("conf2win");
        assert_eq!(
            (page.x, page.y, page.width, page.height, page.anchor),
            (16, 48, 366, 296, 0)
        );
    }

    /// Gated on a retail install. statcom2 is the live Status selector (vault
    /// brief §2): header-declared item count matches the walk, ids run 1..=13
    /// plus one all-blocked trailing block, and its single-column rows keep
    /// Left/Right parked on their own id while Up/Down chain through real
    /// neighbors — the same bytes conf2win uses to step between an option and
    /// its value column (.29's fix rides these, not any axis rule).
    #[test]
    fn real_statcom2_and_conf2win_items_carry_neighbor_laws() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let windows = windows_from_root(&root).expect("menu windows");
        let sel = &windows["statcom2"];
        assert_eq!(sel.item_count, 14);
        let items: Vec<&ItemBlock> = sel.items.iter().flatten().collect();
        assert_eq!(items.len(), 14, "walk must complete the declared blocks");
        for (i, it) in items.iter().enumerate() {
            assert_eq!(it.id as usize, i + 1, "ids equal 1-based position");
        }
        let blocked = items.last().unwrap();
        assert_eq!(
            (blocked.up, blocked.down, blocked.right, blocked.left),
            (None, None, None, None)
        );
        // Single-column selector: L/R parked on own id everywhere else.
        for it in &items[..13] {
            assert_eq!(it.right, Some(it.id));
            assert_eq!(it.left, Some(it.id));
            assert_ne!(it.up, None);
            assert_ne!(it.down, None);
        }
        // conf2win's option/value grid: Right steps across columns.
        let page = &windows["conf2win"];
        let pitems: Vec<&ItemBlock> = page.items.iter().flatten().collect();
        assert_eq!(pitems.len(), 11);
        assert_eq!(
            (pitems[0].id, pitems[0].right, pitems[0].left),
            (1, Some(2), Some(2))
        );
        // Up/Down still move rows even though Left/Right also do: the graph
        // is data (up/down ∈ 1..=11 everywhere, never blocked).
        for it in &pitems {
            assert!(it.up.is_some() && it.down.is_some());
        }
    }

    /// textA/textB law on a synthetic block (loader FFXiMain.dll retail-2026-09
    /// RVA 0x11B6B0, vault brief §7): after parts from +0x20 sit the two
    /// ASCII-decimal ids; `-1` reads as None.
    #[test]
    fn item_blocks_decode_their_ui_string_ids() {
        let mut block = vec![0x40u8, 0x00];
        while block.len() < ITEM_TEXT_A_PART_COUNT_OFF {
            block.push(0);
        }
        block.push(0); // parts = 0
        block.push(0);
        block.extend_from_slice(&[3, 2]); // lenA=3 ("480"), lenB=2 ("-1")
        while block.len() < ITEM_BLOCKS_BODY_OFF {
            block.push(0);
        }
        block.extend_from_slice(b"480\0-1\0");
        let (a, b) = read_block_text(&block, 0, ITEM_TEXT_A_PART_COUNT_OFF);
        assert_eq!((a, b), (Some(480), None));

        // A textB caption id parses too: overwrite the "-1" run with "81".
        let mut block2 = block.clone();
        let n = ITEM_BLOCKS_BODY_OFF + 4; // after "480\0"
        assert_eq!(&block2[n..n + 2], b"-1");
        block2[n..n + 2].copy_from_slice(b"81");
        assert_eq!(
            read_block_text(&block2, 0, ITEM_TEXT_A_PART_COUNT_OFF).1,
            Some(81)
        );
    }

    /// Gated on a retail install. The selector rows carry their labels as UI
    /// string ids resolved through the two catalogs (GetText wrapper FFXiMain.dll
    /// retail-2026-09 RVA 0x152EF0; vault main_menu.md §59): menuwind/socialme
    /// items wear short captions from [`CAPTION_SHORT_CATALOG_FILE_ID`] and
    /// help descriptions from [`CAPTION_CATALOG_TABLE_14_FILE_ID`], read in
    /// DISPLAY (y) order — which puts Synthesis at page-1 row 5 exactly as the
    /// pair-table law of vault §4 says, straight out of block coordinates.
    #[test]
    fn real_selector_captions_and_descriptions_resolve_from_catalogs() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let windows = windows_from_root(&root).expect("menu windows");
        let captions = |file_id| {
            let loc = root.resolve(file_id).expect("catalog file id");
            let bytes = std::fs::read(loc.path_under(&root)).expect("catalog bytes");
            ffxi_msg_table(&bytes)
        };
        let caption_texts = captions(CAPTION_SHORT_CATALOG_FILE_ID);
        let desc_texts = captions(CAPTION_CATALOG_TABLE_14_FILE_ID);

        // Chrome blocks (menuwind ids 13-15 at x=-16/x=113 and the blocked
        // trailing row) carry neither string field; live rows always do.
        let has_strings = |it: &&ItemBlock| it.text_a_id.is_some() || it.text_b_id.is_some();
        let menuwind = &windows["menuwind"];
        let rows: Vec<(&ItemBlock, Option<&str>)> = menuwind
            .items_in_display_order()
            .into_iter()
            .filter(has_strings)
            .map(|it| {
                (
                    it,
                    it.text_b_id
                        .and_then(|i| usize::try_from(i).ok())
                        .and_then(|i| caption_texts.get(i).map(String::as_str)),
                )
            })
            .collect();
        let labels: Vec<&str> = rows
            .iter()
            .map(|(_, c)| c.expect("caption id resolves"))
            .collect();
        assert_eq!(
            labels,
            vec![
                "Status",
                "Equipment",
                "Magic List",
                "Items",
                "Synthesis",
                "Abilities",
                "Party",
                "Trade",
                "Search",
                "Linkshell",
                "Region Info",
                "Map",
            ]
        );

        // statcom2 carries only descriptions (textB is -1), and its display
        // order comes straight from block y: id 12 (Master Levels) sits third.
        let stat = &windows["statcom2"];
        let disp_ids: Vec<i16> = stat
            .items_in_display_order()
            .iter()
            .filter(|i| i.text_a_id.is_some() || i.text_b_id.is_some())
            .map(|i| i.id)
            .collect();
        assert_eq!(disp_ids, [1, 2, 12, 3, 4, 5, 6, 10, 11, 9, 7, 8, 13]);
        let desc_of = |id: i16| -> String {
            let it = stat
                .items
                .iter()
                .flatten()
                .find(|i| i.id == id)
                .expect("statcom2 item");
            it.text_a_id
                .and_then(|a| usize::try_from(a).ok())
                .and_then(|a| desc_texts.get(a).cloned())
                .expect("description resolves")
        };
        assert!(desc_of(9).starts_with("Display total time spent")); // /playtime row
        assert!(desc_of(12).starts_with("View all master levels"));
        // The two evitem rows share their description id (720).
        let ta = |id: i16| {
            stat.items
                .iter()
                .flatten()
                .find(|i| i.id == id)
                .and_then(|i| i.text_a_id)
        };
        assert_eq!(ta(6), Some(720));
        assert_eq!(ta(6), ta(10));
    }

    /// Parse a d_msg catalog's entries into plain strings (same law as
    /// [`crate::dmsg::DmsgTable`], kept local so the test reads as one table).
    fn ffxi_msg_table(bytes: &[u8]) -> Vec<String> {
        let table = crate::dmsg::DmsgTable::parse(bytes).expect("d_msg catalog");
        (0..table.len())
            .map(|i| table.text(i).unwrap_or("").to_string())
            .collect()
    }

    /// Gated on a retail install. The config-family row law is ITEM blocks,
    /// not widgets: `conf5win` dispatches 16 items while carrying only 8
    /// `windowpsNNN` captions, and every item's own textA resolves through the
    /// description catalog (vault main_menu.md §43/§59; bead kuluu-o9qt7.20.1).
    #[test]
    fn real_conf_family_rows_are_items_not_widgets() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let windows = windows_from_root(&root).expect("menu windows");
        let win = &windows["conf5win"];
        assert_eq!(win.option_row_ids().len(), 8, "widget list under-counts");
        assert_eq!(win.live_items().len(), 16);
        assert!(
            windows["configwi"]
                .items
                .iter()
                .flatten()
                .any(|it| it.id == 14 && it.text_a_id.is_none() && it.text_b_id.is_none()),
            "the hub's chrome block carries no UI string"
        );
        assert_eq!(windows["configwi"].live_items().len(), 13);
        let loc = root
            .resolve(CAPTION_CATALOG_TABLE_14_FILE_ID)
            .expect("catalog");
        let table = crate::dmsg::DmsgTable::parse(&std::fs::read(loc.path_under(&root)).unwrap())
            .expect("d_msg");
        let label_of = |it: &ItemBlock| {
            it.text_a_id
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| table.text(i))
                .unwrap_or("")
                .to_string()
        };
        let rows = win.live_items();
        assert_eq!(rows[0].id, 1);
        assert_eq!(
            label_of(rows[0]),
            "Separate the log window into an upper and lower version."
        );
        assert_eq!(
            label_of(rows[6]),
            "Set window pattern to type one. (Default)"
        );
    }

    /// Gated on a retail install. Every live row of the three command-menu
    /// selectors resolves its retail caption/description string from the DATs
    /// in display order — the authority kuluu renders from at runtime (bead
    /// kuluu-1z9qs). The expected vectors below are transcribed once from this
    /// pin; drift here fails CI rather than the user.
    #[test]
    fn real_selector_row_strings_in_display_order() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let windows = windows_from_root(&root).expect("menu windows");
        let captions = |file_id| {
            let loc = root.resolve(file_id).expect("catalog file id");
            let bytes = std::fs::read(loc.path_under(&root)).expect("catalog bytes");
            ffxi_msg_table(&bytes)
        };
        let caption_texts = captions(CAPTION_SHORT_CATALOG_FILE_ID);
        let row_labels = |name: &str| -> Vec<String> {
            windows[name]
                .items_in_display_order()
                .into_iter()
                .filter(|it| it.text_a_id.is_some() || it.text_b_id.is_some())
                .map(|it| {
                    it.text_b_id
                        .and_then(|i| usize::try_from(i).ok())
                        .and_then(|i| caption_texts.get(i).cloned())
                        .unwrap_or_else(|| format!("textB:{:?}", it.text_b_id))
                })
                .collect()
        };
        // The odd retail captions are catalog entries verbatim: the file
        // holds `Magic List`, `Set Bazaar` and `MacroPalette` as distinct
        // NUL-terminated strings (raw XOR-0xFF scan), while kuluu had grown
        // `Magic`, `Bazaar` and an invented `Edit Macros`.
        assert_eq!(
            row_labels("menuwind"),
            vec![
                "Status",
                "Equipment",
                "Magic List",
                "Items",
                "Synthesis",
                "Abilities",
                "Party",
                "Trade",
                "Search",
                "Linkshell",
                "Region Info",
                "Map",
            ]
        );
        assert_eq!(
            row_labels("socialme"),
            vec![
                "Missions",
                "Quests",
                "Key Items",
                "View House",
                "Set Bazaar",
                "MacroPalette",
                "Config",
                "Help Desk",
                "Current Time",
                "Communication",
                "Shut Down",
                "Log Out",
            ]
        );
    }

    /// Gated on a retail install. The navigation law (vault brief §5: each
    /// direction reads its own neighbor byte — loader FFXiMain.dll retail-2026-09
    /// RVA 0x11B6B0): following each live row's Down byte from the topmost item
    /// visits exactly the y-sorted display sequence on all three command-menu
    /// selectors. That is what makes kuluu's list ±1 cursor provably equivalent
    /// to retail's per-byte chain on single-column selectors (bead kuluu-o9qt7.29).
    #[test]
    fn real_selectors_down_chain_visits_rows_in_display_order() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let windows = windows_from_root(&root).expect("menu windows");
        for name in ["menuwind", "socialme", "statcom2"] {
            let win = &windows[name];
            let live: Vec<&ItemBlock> = win
                .items_in_display_order()
                .into_iter()
                .filter(|it| it.text_a_id.is_some() || it.text_b_id.is_some())
                .collect();
            let mut by_id: std::collections::BTreeMap<i16, &ItemBlock> =
                live.iter().map(|it| (it.id, *it)).collect();
            let want: Vec<i16> = live.iter().map(|it| it.id).collect();
            // The chain is CIRCULAR: the last row's Down byte wraps to the
            // first (measured on all three selectors), so drop visited ids from
            // the map — starting with the seed row.
            by_id.remove(&want[0]);
            let mut got = vec![want[0]];
            let mut cur = live[0];
            while let Some(next) = cur.down {
                // Own-id byte means parked; removing each visited id also
                // guards against any loop in the data.
                let Some(nxt) = by_id.remove(&next).filter(|_| next != cur.id) else {
                    break;
                };
                got.push(next);
                cur = nxt;
            }
            assert_eq!(got, want, "{name}: Down-byte chain == display order");
        }
    }

    /// The same-file collision law: a block-less twin chunk must not shadow the
    /// record that actually carries blocks, whichever order they appear in.
    /// Measured on retail-2026-09 `ROM/119/51.DAT`, where four roots have such a
    /// pair (see `windows_from_root`'s doc).
    #[test]
    fn a_block_less_twin_never_shadows_the_record_with_blocks() {
        let rich = MenuWindow {
            fourcc: "cfil".into(),
            root: "cfilter".into(),
            offset: 0x2819B0,
            size: 0x400,
            widgets: Vec::new(),
            item_count: 14,
            frame: Some(FrameBlock {
                x: 16,
                y: 48,
                cursor_a: 0,
                cursor_b: 0,
                width: 366,
                height: 248,
                anchor: 0,
            }),
            items: vec![Some(ItemBlock {
                id: 1,
                x: 34,
                y: 21,
                width: 256,
                height: 16,
                up: None,
                down: Some(2),
                right: Some(2),
                left: None,
                text_a_id: None,
                text_b_id: None,
            })],
        };
        let stub = MenuWindow {
            fourcc: "cfil".into(),
            root: "cfilter".into(),
            offset: 0x2AAC30,
            size: 0x80,
            widgets: Vec::new(),
            item_count: 77,
            frame: None,
            items: Vec::new(),
        };

        for (first, second) in [(&rich, &stub), (&stub, &rich)] {
            let mut merged = MergedWindows::default();
            merged.merge(7, [first.clone()]);
            merged.merge(7, [second.clone()]);
            let windows = merged.into_windows();
            let kept = &windows["cfilter"];
            assert_eq!(
                kept.offset, rich.offset,
                "rich record survives whichever chunk order (first={first:#?})",
            );
            assert!(kept.frame.is_some(), "the frame block is the law-bearer");
        }

        // Across files the newest generation still wins outright (§47/§49 law).
        let mut merged = MergedWindows::default();
        merged.merge(LEGACY_UI_DAT_FILE_ID, [rich.clone()]);
        merged.merge(NAMED_ROOT_UI_DAT_FILE_ID, [stub.clone()]);
        assert_eq!(merged.into_windows()["cfilter"].offset, stub.offset);
    }

    /// Gated on a retail install. The roomlist remodel driver (.text FFXiMain.dll
    /// retail-2026-09 0x1D6900) fetches its five row captions and the remodel
    /// confirm string via GetText(table 12, ids 450/446..449 + 451); the install
    /// carries them in the catalog at [`GETTEXT_TABLE_12_CATALOG_FILE_ID`] (n=526).
    /// Transcribed once by run — drift fails CI rather than the player (bead
    /// kuluu-o9qt7.13.5; vault main_menu.md §59 addendum 21).
    #[test]
    fn real_roomlist_caption_catalog_entries_match_the_dll_fetches() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let loc = root
            .resolve(GETTEXT_TABLE_12_CATALOG_FILE_ID)
            .expect("table-12 catalog file id");
        let bytes = std::fs::read(loc.path_under(&root)).expect("catalog bytes");
        let table = crate::dmsg::DmsgTable::parse(&bytes).expect("d_msg catalog");
        assert_eq!(table.len(), 526, "measured entry count (JP/EN twins alike)");
        // Row pool order as built at .text 0x1D695E..: style words {0, 615, 616,
        // 617, 618} paired with caption ids fetched in the same write order
        // (`push 0x1c2` first at .text 0x1D6907 — id 450, NOT the chapter-counter
        // strings 457.. that live next door in this catalog).
        assert_eq!(
            table.text(450),
            Some("No changes"),
            "row 0 (style word 0 — select refreshes locally, never sends)"
        );
        assert_eq!(table.text(446), Some("San d'Orian style"));
        assert_eq!(table.text(447), Some("Bastokan style"));
        assert_eq!(table.text(448), Some("Windurstian style"));
        assert_eq!(table.text(449), Some("Mog Patio"));
        assert_eq!(
            table.text(451),
            Some("Make changes?"),
            "the dialog the row callback raises (.text FFXiMain.dll retail-2026-09 RVA 0x1D6B85)"
        );
    }

    /// Gated on a retail install. The `/furcc` and `/layoutctrl` slash handlers (.text
    /// FFXiMain.dll retail-2026-09 `.text 0x7E9B0`/`.text 0x7EA40`) emit ids 0x12D..0x134
    /// through the chat emitter (`.text 0x129DE0`), which resolves them in GetText **table
    /// 9**; this install carries that catalog at [`GETTEXT_TABLE_9_CATALOG_FILE_ID`]. The
    /// option store is inverted for `/furcc` — value 0 IS collision ON — and the catalog
    /// wording proves it (`turned on.` rides the `on` arm's write of 0; vault main_menu.md
    /// §59 addendum 35, bead kuluu-o9qt7.41).
    #[test]
    fn real_table9_chat_message_catalog_carries_the_option_toggle_lines() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let loc = root
            .resolve(GETTEXT_TABLE_9_CATALOG_FILE_ID)
            .expect("table-9 catalog file id");
        assert_eq!(
            (
                loc.path_under(&root).parent().unwrap().file_name().unwrap(),
                loc.path_under(&root).file_name().unwrap()
            ),
            (std::ffi::OsStr::new("165"), std::ffi::OsStr::new("72.DAT")),
            "FTABLE word law points table 9 at ROM/165/72.DAT"
        );
        let bytes = std::fs::read(loc.path_under(&root)).expect("catalog bytes");
        let table = crate::dmsg::DmsgTable::parse(&bytes).expect("d_msg catalog");
        assert_eq!(
            table.len(),
            348,
            "measured entry count (EN; JP twin ROM/165/56 identical)"
        );
        assert_eq!(table.text(0xC3), Some("Q&A"), "§34's pairing pin");
        // `/furcc` — setter arms .text 0x7E9F2 (value 1, msg 0x12E) and 0x7EA13 (value 0,
        // msg 0x12D); query getter arm reports 0x130 for non-zero, 0x12F for zero.
        assert_eq!(
            table.text(0x12D),
            Some("Furnishing camera collision has been turned on."),
            "written by the `on` arm alongside value 0 — store inverted"
        );
        assert_eq!(
            table.text(0x12E),
            Some("Furnishing camera collision has been turned off.")
        );
        assert_eq!(
            table.text(0x12F),
            Some("Furnishing camera collision is currently on.")
        );
        assert_eq!(
            table.text(0x130),
            Some("Furnishing camera collision is currently off.")
        );
        // `/layoutctrl` — pair table .data 0x357AA0 = {type1, type2}: setter arms value 0 /
        // msg 0x131 and value 1 / msg 0x132; query reports 0x133 (zero) / 0x134 (non-zero).
        assert_eq!(
            table.text(0x131),
            Some("Layout control style has been set to type 1.")
        );
        assert_eq!(
            table.text(0x132),
            Some("Layout control style has been set to type 2.")
        );
        assert_eq!(
            table.text(0x133),
            Some("Layout control style is currently set to type 1.")
        );
        assert_eq!(
            table.text(0x134),
            Some("Layout control style is currently set to type 2.")
        );
        // `/statusparty` — handler .text FFXiMain.dll retail-2026-09 `.text 0x7EAE0` picks these
        // two from the WRITTEN value (1 → enabled, 0 → disabled) at its shared message tail.
        assert_eq!(
            table.text(0x138),
            Some("Party member status icons are enabled.")
        );
        assert_eq!(
            table.text(0x139),
            Some("Party member status icons are disabled.")
        );
    }

    #[test]
    fn chunk_chain_stops_at_a_bogus_header() {
        let mut bytes = chunk(b"con7", root_record("conf3win"));
        // A second chunk header claiming a size past EOF must end the walk.
        let n = bytes.len();
        bytes.resize(n + 16, 0);
        bytes[n..n + 4].copy_from_slice(b"bad!");
        bytes[n + 4..n + 8].copy_from_slice(&(999_999u32 << 7 | 0x30u32).to_le_bytes());
        let windows = parse_windows(&bytes);
        assert_eq!(windows.len(), 1);
    }

    /// The myroom door row's four catalog ids answer in this install, and they pair the way the
    /// window's update slot pairs them: the caption comes from GetText table 0x0E (`+0x4C` setter),
    /// the help line from table 0x0D (`+0x50` setter).
    #[test]
    fn real_moghouse_door_caption_and_help_pairs_resolve() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let catalog = |file_id| {
            let loc = root.resolve(file_id).expect("catalog file id");
            let bytes = std::fs::read(loc.path_under(&root)).expect("catalog bytes");
            crate::dmsg::DmsgTable::parse(&bytes).expect("d_msg catalog")
        };
        let captions = catalog(CAPTION_CATALOG_TABLE_14_FILE_ID);
        assert_eq!(
            captions.text(MOGHOUSE_DOOR_CAPTION_CLOSED_ID),
            Some("Open your Mog House to party and alliance members.")
        );
        assert_eq!(
            captions.text(MOGHOUSE_DOOR_CAPTION_OPEN_ID),
            Some("Close your Mog House to visitors.")
        );
        let helps = catalog(CAPTION_SHORT_CATALOG_FILE_ID);
        assert_eq!(
            helps.text(MOGHOUSE_DOOR_HELP_CLOSED_ID),
            Some("Open Mog House")
        );
        assert_eq!(
            helps.text(MOGHOUSE_DOOR_HELP_OPEN_ID),
            Some("Close Mog House")
        );
    }
}
