# Items window (main menu > Items) — retail observations

Observed 2026-09-11 on HorizonXI (retail client, Ashita loader), character
Oldman in Selbina with 59 items in a 60-slot inventory and no other bags.
Captures: `artifacts/retail/items-*.png` and `artifacts/retail/items2-*.png`
(local only). Kuluu implements this record in `kuluu-render/src/hud/item_screen.rs`,
`hud/menu_help_bar.rs`, `hud/item_detail.rs`, `hud/menu.rs` (item submenu rows)
and `kuluu/src/view_native/text_input/menu.rs` (list navigation).

## Layout

- **Help bar** (top, full width): `Items 59/60 | Select an item.` — the count is
  held/capacity of the bag, drawn small beside the title.
- **List** docked top-left: 10 rows per page, each a small icon then the item
  name; stackable items overlay the stack count on the icon's top-left corner
  (a plain icon for a single item); a thin scrollbar with a proportional thumb
  runs down the right edge; a pointer glyph sits left of the box on the cursor
  row, which also gets a lighter band.
- **Options box** docked top-right under the Network block: title `Options`,
  sub-label `+ :Sort` (the window-change key's glyph), rows `Auto`, `Manual`,
  `Recycle Bin`. The three rows render identically whichever sort mode is in
  effect; only the cursor row differs.
- **Item card** docked bottom-left above the chat log, in the compass/clock
  slot (both hidden while the list is open): icon, DAT-cased name (`Star orb`),
  Rare/Ex icons top-right, description lines. Equipment adds `All Races` above
  the description and `Lv.N All Jobs` (or the job list) below it, then the
  charge/recast line; consumables show the description alone
  (`AGI+3 VIT-5` / `Duration: 1 hour` are part of the description).

## Navigation

- **Up/Down** move one row and clamp: Up on the first item and Down on the
  last do nothing (no wrap). Leaving the visible page scrolls it by one row;
  the cursor stays on the edge row.
- **Left/Right** move the cursor ten rows and shift the page ten rows in step,
  both clamped. From a page-aligned view the next page shows with the cursor on
  its top row; after a one-row scroll the cursor stays on the bottom row.
  Sequence observed (0-based cursor/page start): 10/1 -> 20/11 -> 30/21 ->
  40/31 -> 50/41 -> 58/49 (clamped) -> Left -> 48/39.
- **Numpad +** (Select active window) toggles focus between the list and the
  Options box; the cursor lands on `Auto`. Up/Down in the box wrap
  (Recycle Bin -> Auto). **Esc** in the box returns focus to the list.
- **Enter** on an item opens a top-right `Item` submenu, `Use` then `Drop`, in
  the Options box's slot while the list stays on screen. Up/Down wrap between
  the two rows. **Esc** returns to the list.
- **Esc** from the list returns to the Commands menu (cursor on Items); a
  second Esc closes to the world.
- The Commands menu reopens on the entry last used (Items), not on Status.

## Help-bar strings (verbatim)

| Focus | Title | Hint |
|---|---|---|
| Commands, cursor on Items | `Items` | `View current inventory.` |
| Item list | `Items 59/60` | `Select an item.` |
| Options: Auto | `Auto-sort` | `Automatically rearrange items.` |
| Options: Manual | `Manual Sort` | `Manually rearrange items.` |
| Options: Recycle Bin | `Recycling Bin` | `Display thrown-away items. They will disappear for good when you change areas, log out, or disconnect.` |
| Item submenu: Use | `Items` (no count) | `Use an item.` |
| Item submenu: Drop | `Items` (no count) | `Dispose of item. Most items will be placed in the recycle bin. If the recycle bin is full, the oldest item in the recycle bin will be deleted.` |

A title wider than the title box (`Recycling Bin`) scrolls in with a marquee
animation the moment it changes.

## Not observed

- What confirming `Drop`, `Auto`, `Manual` or `Recycle Bin` shows next (no
  item was dropped and no sort was confirmed). Kuluu uses a Yes/No confirm
  defaulting to No for Drop and drops the whole stack; unverified.
- Bag tabs / multi-bag switching: this character had only the inventory.
- Whether `Use` on an unusable item shows anything (`Use` was listed for the
  non-usable Star Orb). An earlier capture of a cooling-down item recorded a
  silent refusal (bead kuluu-5ndh).
