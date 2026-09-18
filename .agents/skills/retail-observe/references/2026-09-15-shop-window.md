# NPC shop window (retail client, from binary + packet docs)

Recorded 2026-09-15. This is a **binary/packet-doc observation, not a live
drive** — the Parallels VM was unavailable (see "What is still unpinned").
Everything below is sourced; nothing here was inferred from a screenshot.

The oracle for Kuluu's shop implementation (bead kuluu-7c47). Open work lives
in beads, not here.

## A shop is not an event

Three independent sources say the shop window sits outside the event system:

- LSB's vendors call `showText` (a TALKNUM-family chat line) and then
  `sendMenu(xi.menuType.SHOP)` — never `startEvent`
  (`vendor/server/scripts/globals/shop.lua` `xi.shop.general`).
- `sendMenu` case 2 pushes `GP_SERV_COMMAND_SHOP_OPEN` followed by
  `GP_SERV_COMMAND_SHOP_LIST` (`vendor/server/src/map/lua/lua_base_entity.cpp`).
- The server *refuses* a purchase while the character is in an event:
  `GP_CLI_COMMAND_SHOP_BUY::validate` is `blockedBy({ BlockedState::InEvent })`
  (`vendor/server/src/map/packets/c2s/0x083_shop_buy.cpp`). The same block is on
  `0x084_shop_sell_req.cpp` and `0x085_shop_sell_set.cpp`.

So anything that ends an event must not be what closes the shop, and anything
that opens an event must not be how the shop opens.

## The window has no close packet

The only c2s opcode that could close a shop, `GP_CLI_COMMAND_SHOP_REQ` (0x082),
is marked *"Deprecated: this packet is no longer used"* and *"a special
GM-related packet"* used by `//slist`
(`research/XiPackets/world/client/0x0082/README.md`). The client-to-server shop
vocabulary is exactly 0x083 buy, 0x084 sell-appraise, 0x085 sell-confirm.

**The window's entire lifetime is a client decision.** Nothing tells the server
the player walked away.

## The client's shop table

`research/XIClient/src/XIClient/include/Game/State/GC_SHOP_SYS.h`:

- `GC_SHOP_SYS::Entries` is a fixed **80-entry** array, plus `Status`,
  `SellResult` and `BuyResult` words.
- `GC_ZONE` owns one at offset `0x4644` (`GC_ZONE.h`), so the table is
  **zone-scoped** — a zone change takes the vendor's stock with it.

LSB sends at most **19 rows per 0x03C packet**
(`vendor/server/src/map/packets/s2c/0x03c_shop_list.cpp`), so an 80-row shop is
five packets. Rows must accumulate, not replace.

### How a row's index is derived

`research/XiPackets/world/server/0x003C/README.md`, on `GP_SHOP::ShopIndex`:

> This value is not used by the client. Instead, it starts the base index from
> the main `ShopItemOffsetIndex` value and increments it for each item in the
> packet.

That derived index is what c2s 0x083 sends back as `ShopItemIndex`. `ShopNo` in
the same packet is *"Unused. The client does not set or use this value."*, and
`PropertyItemIndex` is *"always set to 0"*.

`Flags` on 0x03C: *"The client currently only uses the first bit."* LSB sends
`0x00` on a page with more to follow and `0x89` on the last, so bit 0 marks the
final page.

## Menu primitives

`research/XIClient/src/XIClient/source/UI/Windows/PrimMng.cpp` registers four
shop primitives, which is the shape of the UI:

| Primitive | Notes |
|---|---|
| `menu    shop  ` | the ware list; `PrimitiveHandlesCancel` |
| `menu    shopmain` | the contextual picker — "main" makes it the entry point |
| `menu    shopbuy ` | the buy-side confirm box |
| `menu    shopsell` | the sell-side confirm box |

All four carry `PrimitiveHandlesCancel`, i.e. each level answers cancel itself
rather than passing it up — one level unwound per press.

## The sell handshake

`research/XiPackets/world/client/0x0085/README.md`:

> When the client is selling items to a shop, it will first send an `0x0084`
> packet to obtain the sale price that the item is worth. It will then send this
> packet to confirm the sale of the previously selected item. The client will
> always send an `0x0084` packet first when selling an item to ensure the proper
> item is set as the selling item. **Even if the client has already price
> checked an item in the same menu, it will send both packets every time.**

The emphasised sentence implies the retail client also price-checks while
*browsing* the sell list, not only at the moment of sale — unconfirmed, see
below.

Server side: `0x084_shop_sell_req.cpp` silently drops the appraisal for an item
carrying `ItemFlag::NoSale` (`@FLAG_NOSALE = 0x01000` in
`vendor/server/sql/item_basic.sql`), parks the item in the last slot of the shop
container, and answers 0x03D. `0x085_shop_sell_set.cpp` `validate` requires
`SellFlag == 1` *and* a prior 0x084.

s2c 0x03D `Type`: `0` = appraisal, `1` = completed sale
(`research/XiPackets/world/server/0x003D/README.md`). LSB only ever sends
`Type = 0`; a completed sale comes back as `GP_SERV_COMMAND_MESSAGE` plus
`ITEM_SAME` instead, and LSB leaves `Count` at 0
(`vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp` sets only `Price`,
`PropertyItemIndex` and `Type`).

## Interaction range

A shop can only be opened by triggering the NPC, and LSB accepts a Trigger only
within **6.0 yalms**:
`if (distance(PNpc->loc.p, PChar->loc.p) <= 6.0f && ...)`
(`vendor/server/src/map/packets/c2s/0x01a_action.cpp`).

## What is still unpinned

These need a live capture (bead kuluu-kyjh):

- How many ware rows the list draws, and how it pages.
- Where `shopmain` sits relative to the list, and whether the window opens
  focused on the picker or on the wares.
- The verbatim help-bar strings for Buy / Sell / the confirm step.
- Whether the sell list shows a live appraised price per row (the 0x0085 note
  above hints yes).
- Whether retail closes the window when the player is displaced out of the
  vendor's range. Menus plant the character, so the player cannot walk out
  unaided — but knockback, a warp and a vendor despawn all can.

**Why no live drive:** the Parallels VM was paused, and this machine runs
Parallels **Standard** — `prlctl start`/`resume` both refuse ("available only in
Parallels Desktop for Mac Pro or Business Edition"). Opening the `.pvm` bundle
resumed it, but the only window Parallels then exposed was an
"You are running an older version of Parallels Desktop" upsell dialog, which
took neither `observe.sh click` nor System Events keystrokes, and the guest window
never appeared behind it. Dismissing that dialog by hand is the unblock.
