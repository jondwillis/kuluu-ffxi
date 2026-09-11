# Auction House spec (retail FFXI, observed on HorizonXI)

## Contents
- Wire architecture (LSB, authoritative) — including why catalog paging is
  pull-based
- Category tree
- Screens and layout — sell flow, Sales Status, bid (browse) flow
- Interaction model
- Open questions (pin live before implementing the detail)

Observed 2026-08-05 via two user-driven recordings at the Bastok Mines AH counter
(character "Atti", windowed Parallels VM):

- Recording 1 (~128s): Sales Status check, two complete stack sales (Bird Egg,
  Lizard Egg), sell-side price-history browsing, Bid browse of Hand-to-Hand,
  two Price History lookups, three (deliberately low, failed) bids, exit.
- Recording 2 (~52s): category-hierarchy tour (right-edge crop of the browse menu).

Frame-by-frame transcripts (local only, not committed):
`artifacts/retail/ah-obs/transcript_t*.md` (recording 1, 1 fps),
`artifacts/retail/ah-obs2/transcript_t*.md` (recording 2, 1 fps).
User-reported controls not resolvable at 1 fps are marked *(user-reported)*.

This file is the **observation record** — the oracle for Kuluu's AH
implementation (bead kuluu-god). Open work lives in beads, not here.

## Wire architecture (LSB, authoritative)

The AH spans **two transports**:

| Function | Transport | Source |
|---|---|---|
| Browse category item lists | search server TCP, `TCP_AH_REQUEST`/`TCP_AH_REQUEST_MORE` | `vendor/server/src/search/search_handler.cpp` (`HandleAuctionHouseRequest`) |
| Price history | search server TCP, `TCP_AH_HISTORY_SINGLE`/`_STACK` | same (`HandleAuctionHouseHistory`) |
| Open/sell/bid/status/cancel | map c2s `0x04E` / s2c `0x04C` | `vendor/server/src/map/packets/c2s/0x04e_auc.{h,cpp}`, `s2c/0x04c_auc.cpp` |

Map-side commands (`GP_CLI_COMMAND_AUC_COMMAND`): `Open=0x02` (s2c only),
`AskCommit=0x04` (fee quote before sale confirm), `Info=0x05` (Sales Status
open), `WorkCheck=0x0A` (AH open), `LotIn=0x0B` (sale confirm), `LotCancel=0x0C`,
`LotCheck=0x0D` (per-slot sales-status probe, 7 slots), `Bid=0x0E`.
`ItemStacks`: 1 = single, 0 = stack. Sales status is a fixed 7-slot array.

Search transport framing (`search_handler.cpp` `decrypt`/`encrypt`): `[u16 len]`
header, `"IXFF"` magic at 0x04 (s2c), body blowfish-ECB'd in pairs from offset 8,
MD5 integrity hash at `len-0x14`, 4-byte client key at `len-4`. Blowfish key =
MD5 over a **static 24-byte base key** (`search_handler.h`, scrape at build time)
with the packet's trailing 4 bytes spliced in at offset 16 (c2s) and the
decrypted `len-0x18` word at offset 20 for the response. AH list request:
category id at 0x16, sort-param count at 0x12, params at 0x18+8i. History
request: item id at 0x12, stack flag at 0x15. List response (`auction_list.cpp`):
type 0x95 at 0x0B, total count at 0x0E, 20 items/packet of 10 bytes each
(`u16 itemid, u32 single_count, u32 stack_count`) from 0x18; 0x80 at 0x0A marks
the final packet. Those two u32s are open-listing **counts**, not prices —
retail's bracketed `[N]` stock numbers (`data_loader.cpp`
`GetAHItemsToCategory`); prices appear only in sale history.
History response (`auction_history.cpp`): 10 most recent sales.

### Catalog paging is pull-based (measured on HorizonXI 2026-08-05)

The catalog is **not** pushed in one burst. The server answers `TCP_AH_REQUEST`
with page 0 (20 rows) and then sends nothing — each later page must be pulled
with a `TCP_AH_REQUEST_MORE` carrying the same body. Measured against
`play.horizonxi.com`, category 18 (Body): 14 pages, 270 rows, 526 ms when each
page is requested; a wait-only client got page 0 and stalled indefinitely.
The `MORE` frame's category/sort fields are ignored — paging offset is
per-connection server state — but we resend the real ones because stock LSB
re-reads them (`search_handler.cpp:267-268`).

Two consequences for the client:

- **One client key per connection, not per request.** `SearchHandler::key`
  evolves in place; splicing a fresh key into a follow-up request makes the
  next response fail our integrity check. Reuse the key for the whole socket.
- Stock LSB queues *every* page up front, so the same client works there: the
  redundant `MORE` re-dumps land behind the pages still being read and are
  dropped when the socket closes at the final page.

## Category tree (observed; ids from `vendor/server/documentation/Auction Categories.txt`)

Top level, fixed order, one AH-wide root: **Weapons, Armor, Scrolls, Medicines,
Furnishings, Materials, Food, Crystals, Others**. Medicines/Furnishings/Crystals
are leaf categories (no submenu). Every list fits without scrolling; submenus
replace the parent list in place (same anchor, height = entry count).

- **Weapons** (15): Hand-to-Hand, Daggers, Swords, Great Swords, Axes, Great
  Axes, Scythes, Polearms, Katana, Great Katana, Clubs, Staves, Ranged,
  Instruments, Ammo&Misc.
  - **Ammo&Misc.** (4): Ammunition, Fishing Gear, Pet Items, Grips
- **Armor** (11): Shields, Head, Neck, Body, Hands, Waist, Legs, Feet, Back,
  Earrings, Rings
- **Scrolls** (7): White Magic, Black Magic, Songs, Ninjutsu, Summoning, Dice,
  Geomancy
- **Materials** (8): Smithing, Goldsmithing, Clothcraft, Leathercraft, Bonecraft,
  Woodworking, Alchemy, Alchemy 2
- **Food** (3): Meals, Ingredients, Fish
  - **Meals** (8, from LSB ids 52–58 — not toured on video): Meat & Eggs,
    Seafood, Vegetables, Soups, Breads & Rice, Sweets, Drinks
- **Others** (8): Misc., Misc. 2, Misc. 3, Beast-made, Cards, Ninja Tools,
  Cursed Items, Automaton

Note the display-name divergences from LSB's enum comments: retail says
"Ranged" (LSB `BOW`), "Daggers"/"Swords"/… pluralized, "Geomancy" (LSB
`GEOMANCER`). The armor order is retail's equip-slot order (Neck 3rd, Waist
6th), NOT the LSB id order (which has Neck=22, Waist=23 after Feet).

## Screens and layout

Persistent chrome while the AH is open:

- **Top help bar** (full width): `<mode label> | <context help>` with "Help" at
  the right end. The label tracks the active screen: `Auction` → `Items` /
  `Price Set` / `Bid` / `Weapons` / `Price History`. The help text always
  mirrors the highlighted entry (each menu row has its own sentence). During
  async server ops the center shows an animated dot spinner:
  `··●● Downloading data .. ●●··` or `··●● Placing bid ... ●●··`. The bar dims
  (or blanks) while a modal Yes/No dialog has focus.
- **AH root menu** (docked top-right under the Network widget, panel titled
  "Auction"): `Bid` / `Sell` / `Sales Status`. Help text per row: Bid = "View
  all merchandise up for auction.", Sell = "Place unwanted items on auction.",
  Sales Status = "Check your items currently placed on auction." This panel
  persists until deeper right-side menus replace it in place (category lists,
  per-item context menu, sort menu all occupy the same dock).
- Windows animate open/closed in ~1 frame (sliding dark-blue fragments +
  translucent ghost panel).

### Sell flow

1. **Item list** (top-left, 10 rows, icon + name, scrollbar on right edge;
   sellable inventory only). Live **item description pane** below the list
   follows the cursor (name, flavor text, stats, e.g. "Bird egg / This egg is
   renowned for its flavor. / HP+6 MP+6 / Duration: 5 minutes"). Help:
   "Select an item."
2. Selecting an item → **Price Set** screen ("Price Set | Specify the price."):
   - **Price-history window** replaces the list. Header:
     `<Category> :  <icon> <Item Name>  [12]  [N]` — the `12` appears only when
     listing a stack; `[N]` is the count currently listed on the AH. Rows (up
     to 10): `date  seller → buyer  price G`, dates as `YY/M/D` (e.g. `26/8/4`),
     long names ellipsized (`Firedra…`).
   - **Price entry bar** below: left box "Current Gil <coin icon> / 80,147 G";
     right box a digit-spinner: `All ◄ [0] G ▶ 0` over `/999,999,999 G`
     (entered value vs cap), magenta `▲ +` above and `▼ −` below the active
     digit, `◀/▶` move the digit column ("All" = whole-value column). The
     active digit field is tinted red/pink; just-edited digits render orange.
3. Confirming the price → **fee confirm dialog** (lower-left Yes/No): "The
   total transaction fee for / a set of 12 items is 9 gil." (singles: "for
   this item"). Cursor defaults to **Yes**. Fee scales with price (9 gil at
   1,180; 17 gil at 2,670 — LSB `AH_BASE_FEE_*` + `AH_TAX_RATE_*`, clamped by
   `AH_MAX_FEE`; fee is charged on *listing*, observed as an immediate gil drop).
4. → **placement confirm dialog**: "Place bird eggs / up on auction for 1,180
   gil?" Yes/No. (Cursor observed on No at t018 and on Yes at t032 — default
   unresolved at 1 fps; verify live before pinning.)
5. Chat echoes (white): the fee line, then "Merchandise placed on auction."
   plus a 3-line policy explainer: "If merchandise remains unsold after 30
   weeks (Vana'diel time), it will be returned to your current residence." /
   "If a successful bid is made, the proceeds from the sale will be delivered
   to your current residence." / "Signed items will lose their signature after
   being purchased."
6. After a sale the item list refreshes (stack removed, next item revealed).
   Backing out of Price Set flashes help text "Cancel search."; the list
   re-opens with mode label "Items".

Error (attempting to sell a partial stack): red/magenta chat line "You can only
place a single item or a set of 12 such items on auction."

### Sales Status

Opens a top-left window with a column of **7 item slots** (matching the 7-slot
wire array) and body text "You have no items up for auction." when empty. Chat
echo: "You have 0 items listed for sale." (HorizonXI also printed "No results
for page: 1 of 4." / "Current page: 1 of 4. Showing 0 items." — likely
server-custom, verify against LSB before treating as retail.)

### Bid (browse) flow

1. Root → `Bid` → **category menu** in the right dock (9 entries), each with
   help text ("View weapons on auction."). Drill into subcategory (help e.g.
   "Knuckles, claws, and other hand-to-hand weapons.").
2. Leaf category → "Downloading data .." spinner (all panes closed) → **catalog
   list** (top-left, 10 rows/page): icon + name + right-aligned bracketed stock
   count `[14]`; **no bracket = none currently listed** (rows still shown and
   selectable for Price History). The right dock becomes a **category tab
   stack** (all sibling subcategories; active one in gold). Item detail pane
   below the list shows full stats for the highlighted row (equipment renders
   `DMG:+N Delay:+N`, stat lines, `Lv.N JOB/JOB/…`, element glyphs, latent/
   additional-effect lines); the pane blanks briefly while item data loads.
   Mode label "Bid", help "Select an item."
3. Confirming a row → **per-item context menu** in the right dock:
   `Price History / Bid / Sort`. Help per row: "View recent sales data for this
   merchandise." / "Place a bid on this merchandise." / "Rearrange the order of
   listed items."
4. **Price History**: "Downloading data .." → table titled "Price History",
   help "Sales data for the last ten transactions of selected merchandise."
   Up to 10 rows `date seller → buyer price G`, fewer if fewer sales exist.
   Esc returns to the catalog with cursor/page state preserved.
5. **Bid**: → "Auction | Place a bid on this merchandise." (list drawn
   unselected) → **Price Set** variant: item detail pane docks top-left,
   "Current Gil / 80,121 G" panel, digit spinner as in Sell but capped at
   current gil (`/80,121 G`). Confirm → "Bid | Placing bid ..." spinner while
   the list redraws → outcome:
   - Failure (bid below ask): white chat "You were unable to buy the cat
     baghnakhs for 490 gil." (item name in green); list stays focused on the
     same row. No dialog.
   - (Success not captured on video: retail delivers the item and prints an
     acquisition line — pin via live observation when implementing.)
6. **Sort** (equipment categories): submenu `Reset List / By Damage / By Delay
   / By Level / Job / Race` with help per row ("Sort by highest damage
   rating.", "Sort by shortest delay.", "Sort by highest required level.",
   "Narrow down list to weapons usable by current job." / "…by your race.").
   Non-equipment categories (e.g. Food) offer only Alphabetical vs. the
   default order *(user-reported)*.

**"Natural/Unsorted" =** ascending item-ID order. LSB
`HandleAuctionHouseRequest` maps sort params (2=level, 5=damage, 6=delay,
9=name→`item_basic.sortname`; Job/Race filter client-side) and always appends
`item_basic.itemid` as the final `ORDER BY` key — with no params the catalog
is simply item-number order. "Reset List" clears params.

## Interaction model

- Entirely keyboard-first: `▶` blinking cursor at the focused row's left edge,
  row highlight (orange/gold pill in right-dock menus; purple/magenta bar in
  item lists). Up/Down move one row and **wrap top↔bottom**; cursor position is
  remembered when backing out of a submenu. Enter confirms, Esc backs out one
  level (Esc from Price Set shows "Cancel search.").
- Left/Right = page up/page down in item lists *(user-reported; consistent
  with the multi-page jumps seen between frames)*. In the price spinner,
  Left/Right move the digit column (`All ◀ … ▶`), Up/Down step the digit.
- Mouse can click to select menu entries *(user-reported; observed hover at
  recording-2 t052)* — but the whole recorded session was keyboard-driven.
- Every AH action echoes a white chat line; errors are red/magenta.
- Gil renders comma-grouped with a ` G` suffix everywhere.

## Open questions (pin live before implementing the detail)

1. Default cursor position on the placement-confirm dialog (Yes vs No).
2. Successful-bid choreography: chat line text, item delivery, list refresh,
   stock-count decrement.
3. Stack-vs-single choice UI when bidding on an item listed in both forms
   (the `TCP_AH_HISTORY_SINGLE` vs `_STACK` split implies a chooser; not
   captured).
4. Sales Status with items listed: slot rendering, `LotCancel` flow, and the
   "Item sold" state (LSB `Parcel.Stat` 0x02/0x03…).
5. Sort menu contents for non-equipment categories (only *(user-reported)*
   "Alphabetical" + default so far) and armor/food sort help strings.
6. Whether the "page: 1 of 4" Sales Status chat lines are HorizonXI-custom.
