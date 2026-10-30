# Equipment sets: contents, apply arms and the wire, 2026-10-25

How the retail client stores an equipment set locally and what it puts on the wire to use
one. Established by reading the installed client's whole `FFXiMain.dll` recompiled to C
(KNOWN_CLIENTS row `retail-2026-10`, DLL build 2026-10-05) and cross-checked against
LandSandBoat's packet headers, validators and handlers in this checkout. No live client
session was run; nothing here came from the screen.

## The set record and its contents half

Each equipment set is one 80-byte record inside the player's own per-profile set file: a
display name occupying bytes 0..15 (fifteen usable characters, NUL terminated) and the
worn-equipment contents occupying bytes 16..79. The contents are **sixteen entries of four
bytes each**, and the position of an entry in the array *is* the equipment position it
describes:

| entry field | width | meaning |
| --- | --- | --- |
| byte 0, bits 0 and 1 | two flags | bit 0 = this entry has an item; bit 1 = the remove-item flag |
| byte 0, bits 2..7 | 6-bit value | `Category`: which container the item lives in (inventory or a wardrobe), validated as such server side |
| byte 1 | u8 | `ItemIndex`: index of the item inside that container |
| bytes 2 and 3 | u16 | `ItemNo`: the item id |

That is byte for byte `equipsetrequestitem_t` in
[`vendor/server/src/map/packets/c2s/0x052_equipset_check.h`](../../../../vendor/server/src/map/packets/c2s/0x052_equipset_check.h),
including the flag bit order, so the record half and the wire item struct are one shape. An
empty entry reads as all zero; clearing entries keeps the two flag bits and zeroes `ItemIndex`
and `ItemNo`.

## The client applies nothing locally

Selecting a set and choosing its equip option does **not** touch the player's equipment in
the client: it sends c2s 0x051 and waits for the server to answer with normal equipment
updates. There is no client-side equip-from-set path, so kuluu cannot "apply a set" by
editing local state - Equip, Use Current and Edit are all wire work plus file writes.

The twenty-slot structure that sits near the record store in memory is *not* an equipment
list either: it is a one-page cache of loaded records (twenty records keyed by set number,
with an empty sentinel) used to avoid re-reading the set file on every access.

## Set selection state

Two bytes of the set-manager object hold the cursor: slot within page, then page index.
Every option recomputes the set number as `page * 20 + slot`, valid from 0 to 199; an
out-of-range number makes the record accessors fail with no record rather than clamp. Set
files are paged at twenty records per file, which is what that page index addresses.

## The eight option rows and what each does

The options window dispatches on a switch value (not necessarily the display row order):

| switch | retail option | effect | packet |
| --- | --- | --- | --- |
| 0 | Edit (inferred) | hands the record's contents to an editor service and registers a completion callback | none directly; the editor then reports through c2s 0x052 |
| 1 | Change Name (inferred) | opens a small sub-window seeded with the record's current name | none |
| 2 | Copy | copies name plus contents of this record into an 80-byte clipboard buffer | none |
| 3 | Paste (asks first) | installs the clipboard name and contents over this record | none |
| 4 | Delete / reset (asks first) | regenerates the default name for that set number from a fixed template and clears all sixteen entries | none |
| 5 | Use Current (asks first) | wipes the contents, then fills them by walking what the player is wearing now: flag bit 0 set, container taken from the worn item, `ItemIndex` and `ItemNo` written into the matching entry position | none |
| 6 | Equip | validates every populated entry against owned items (fixing or dropping bad ones), then sends the set to be worn | c2s 0x051 |
| 7 | Style Lock | copies the contents, keeps only positions 0..8, and asks the server to lock style to them | c2s 0x053 with Mode = Set |

Which numeric switch value belongs to Edit versus Change Name is inferred from what each one
passes along (contents for one, name for the other); the six others are pinned by their own
effects. The on-screen wording order was not re-established here.

## c2s 0x051 GP_CLI_COMMAND_EQUIPSET_SET - wearing a set

Sent by switch value 6 after validation. Body layout (LSB names, header cited): one `Count`
byte at packet offset 4, three pad bytes, then `Count` four-byte entries starting at packet
offset 8, each `{ ItemIndex, EquipKind, Category }`. The client fills `ItemIndex` and
`Category` straight from the record entry unchanged and puts the **entry position** in
`EquipKind`, which the server validates as a slot type; total size is fixed by the sender at
72 bytes regardless of `Count`, and `Count` never exceeds sixteen. The server performs the
equip calls, so appearance updates arrive back from it.

Before sending, the client runs every entry through slot-validity and ownership checks: an
entry whose slot is unusable or whose item is not owned gets its populated flag cleared in
the working copy (not in the stored record), and unresolvable ones are reported to the player
with system message text rather than sent as-is.

## c2s 0x053 GP_CLI_COMMAND_LOCKSTYLE - style lock

Mode byte at packet offset 5 selects the operation: Disable, Continue, Query, Set, Enable. The
set-driven one is Mode = Set from switch value 7, with `Count` at offset 4 and a Flags byte at
offset 6 (one bit toggled by a caller argument), then up to sixteen eight-byte items starting
at packet offset 8: `{ ItemIndex, EquipKind, Category, pad, ItemNo (two bytes), pad }`.

**Style lock only ever considers the first nine positions.** The client tests each entry's
position against a fixed table of values equal to 0..8 and skips everything else, so a set may
carry sixteen entries and still lock style on at most nine. This matches the server dropping
any item whose `EquipKind` is past the feet slot and testing it against the item's own
equipment-slot mask. The enable / disable / query variants carry no items (`Count` = 0), and
the config rows that turn the feature on or off re-persist their stored value once the packet
is accepted - see the existing announce-law note for that stored byte; the same helper family
sends these mode-only packets.

## c2s 0x052 GP_CLI_COMMAND_EQUIPSET_CHECK - editing a set's items

The set editor keeps its own sixteen-entry copy of contents in exactly the record format, and
its row handler works on that copy: a row action either clears one position or replaces it
with a candidate entry (comparing flag/category bytes and the item id word first, so choosing
the same thing again removes it instead). Only the branch carrying a candidate entry reaches
the wire. What is sent: `EquipKind` at packet offset 4 = the edited position (validated as
0..15), then a single **item-change** entry at packet offset 8 in the four-byte contents shape
(the entry just placed or removed, with its flag bits normalized the same way as the list
entries), followed by the whole sixteen-entry current list starting at packet offset 12. So an
edit is reported as "here is what changed and here is the full set now", and the client still
does not wear anything from it.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| the contents half of a record and the wire item struct are one shape (sixteen four-byte entries, flag bits first) | ffxi-proto/src/equipset.rs EquipSetEntry | matches |
| nothing is worn locally; every equip option is wire work plus file writes | kuluu/src/view_native/text_input/menu.rs wear_equipment_set | matches |
| before sending a set, each populated entry is validated for slot usability and ownership against what the player holds, bad ones are repaired or dropped in the working copy only, and the stored record is left alone | kuluu/src/view_native/text_input/menu.rs wear_equipment_set | matches |
| style lock considers only positions through the feet slot (nine of sixteen) | ffxi-proto/src/equipset.rs lockstyle_admissible_count | matches |
| an edit reports one changed entry plus the whole sixteen-entry list, and wears nothing | ffxi-proto/src/equipset.rs equipset_check_body | matches |
| Use Current wipes the contents half and refills it from what is worn now, with no packet sent | kuluu/src/view_native/text_input/menu.rs use_current_into_set | matches |
| Copy holds a whole-record clipboard (name plus contents), Paste installs both over the target record | kuluu-render/src/hud/client_user_files.rs copy_equipment_set | matches |
| Delete resets the name from a fixed template keyed on set number plus one and clears all sixteen entries | kuluu-render/src/hud/client_user_files.rs delete_equipment_set | matches |
| two cursor bytes (slot within page, then page) with the record number recomputed as page times twenty plus slot, valid zero to one-ninety-nine, out-of-range failing rather than clamping | kuluu-render/src/hud/client_user_files.rs set_page_of | differs: kuluu computes page and slot from a flat index directly, so no two-byte cursor state exists to keep in step |
| an empty entry reads as all zero, and clearing keeps the two flag bits while zeroing item index and item id | ffxi-proto/src/equipset.rs EquipSetEntry::cleared | matches |

## Provenance

Method: generated C recompile of `FFXiMain.dll`, build 2026-10-05, image base `0x10000000`;
looked up per [vendor/rea/INSTALL.md](../../../../vendor/rea/INSTALL.md) as `f_<VA>` with grep over
`research/xi-on-anything/generated/all`, plus `tools/newbuild.py dis --label 2026-10-05` for
instruction verification (e.g. at `0x101a8a60` and the sender loop `0x100e54b8`). Findings carry
provenance "generated C, build 2026-10-05".

Option handler and dispatch:

- Options/select handler `f_101a8190`, reached only through vtable `.rdata` `0x10337538` (its
  entry word is the dword at VA `0x10337550`, i.e. vtable offset 6). Sibling vtables in the same
  object family are installed by constructors `f_101a6c10` (`0x103374f0`), `f_101a7280`
  (`0x10337538`) and `f_101a8d60` (`0x103375a0`, `0x103375e8`); other handler slots in those rows:
  `f_101a6c60`, `f_101a6c90`, `f_101a6d00`, `f_101a6e00`, `f_101a6eb0`, `f_101a6d60`,
  `f_101a7010`, `f_101a72b0`, `f_101a72e0`, `f_101a73b0`, `f_101a83e0`.
- Eight-arm switch: data jump table at VA `0x101a8394` (inside `.text`; `dis` there decodes as
  non-code, confirming it is a table). Arms in order: `0x101a81d0`, `0x101a8209`, `0x101a8216`,
  `0x101a824f`, `0x101a82aa`, `0x101a82e7`, `0x101a8327`, `0x101a835c`.
- Manager globals: set manager object from `dword 0x106627cc` (cursor bytes at offsets `0x20` =
  slot, `0x24` = page); second manager instance `dword 0x106627d4`; clipboard buffer global
  `dword 0x106627e8`; snapshot clock globals `dword 0x106627e0`, `dword 0x106627e4` (written from
  the clock helper `f_100dc120`).

Records, cache and names:

- Accessors: record base `f_101a8a40`, contents half `f_101a8a60` (returns record pointer plus
  `0x10`), both range-checking with `cmp al, 0xc8` then tailing to locator `f_101a8470`. Locator:
  cache scan over `[obj + 0x644 + i*2]`, `i < 20`, empty sentinel `0xff`, record store at
  `[obj + 4]` with stride `0x50`, load path `f_101a8530` (calls page picker `f_101a84e0` and file
  helpers `f_101b9880` / `f_101b9a00` / `f_101b9b00`, left closed), cache insert `f_101a86c0` with
  chain surgery in `f_101a8610`, anchor byte at `obj + 0x66c`, reset `f_101a83c0`. Whole-record
  copy helper `f_101a85d0` has exactly one caller, the cache insert. Per-page object getter
  `f_101a8960` (index under twenty). Name setter with the fifteen-character guard: `f_101a73f0`.
- Contents entry access pattern in consumers: `[base + i*4]`, byte 0 masked with `0x3` for flags,
  shifted right by two for category, byte at `+1`, u16 word at `+2`.

Arms (switch value → target → evidence):

- 0 → `f_10134d90`: passes the contents pointer; lazily builds a service object with vtable
  `.rdata 0x10333830` and performs an indirect call through `[vtable + 0x58]`; registers callback
  literal `f_10134d70` via `f_100e5e40`; service global `dword 0x105781bc`.
- 1 → `f_101a70a0`: allocates `0x6b4`, copies the record name (getter `f_101a8960` twice), then
  attaches a sub-window through globals `dword 0x10662da0` with helpers `f_101c68a0` and
  `f_101c6870`.
- 2 → `f_101a7440`: allocates the clipboard as exactly `0x50` bytes at `dword 0x106627e8`, runs the
  name through the setter, then copies record offsets `0x10..0x4f` into it (direction: record → clipboard).
- 3 → confirm text id `0x115`, callback literal `f_101a7650` → body `f_101a74a0`: setter call from the
  clipboard, then copies clipboard offsets `0x10..0x4f` into the record (direction: clipboard → record).
- 4 → confirm text id `0x116`, callback literal `f_101a7690` → `f_101a75a0` → tail `f_101a7530`: name
  built with the sprintf-style helper `f_10312031` from a fixed template string at VA `0x1037ab20`
  with argument set-number plus one, then contents cleared by `f_101a74f0(ptr, 0)`.
- 5 → confirm text id `0x117`, callback literal `f_101a7670` → body `f_101a75c0`: clears via
  `f_101a74f0(record + 0x10, 2)`, then loops up to sixteen times over currently worn items via
  `f_100e5290` (iterator advance `f_100e52c0`), writing at `[record + 0x10 + entry*4]`: flags
  cleared of bit 1 and set with bit 0, category bits from byte `[esi+5] << 2`, index from byte
  `[esi+4]`, item id u16 from `f_100eb490`.
- 6 → `f_101a7af0`: contents copy and in-place normalization by `f_101a7930` (per-entry slot check
  `f_101a76c0`, lookups `f_101a7710`, `f_101a7810`, `f_101a78a0`, system message text ids `0x112` and
  `0x113`), then sender `f_100e5420`; scratch entry stride eight at `[esp + 0xd4]`.
- 7 → `f_101a7df0`: gate `f_101a7d00`, bail feedback helper `f_10129d90` with sub-window id `0x22`;
  contents copied by `f_101a7d70` (sixteen dwords); per-position gate `f_101a7da0` = membership in the
  table `{0,1,4,5,6,7,8,2,3}` (exactly positions 0..8); scratch entries at `[esp + 0xd4]`, stride 8:
  byte 0 = position, byte 4 = index, byte 5 = category, u16 at 6 = item id; then sender `f_100e5490`;
  success tail re-persists store `0xbf` through wrapper `f_10193830(1, 0xbf)` - the same helper family
  that kuluu's announce-mode note cites at `0x1a8030`.

Packet senders (all use alloc `f_100fc3c0` then commit `f_100fc4d0`; swept by listing every alloc site):

- `f_100e5340` id `0x50`: single equip change, bytes written at packet offsets 4/5/6; caller `f_101327d0`.
- `f_100e5380` id `0x52`, commit size `0x4c`: `[pkt+4]` = first arg (edited position), item-change entry at
  `[pkt+8]` copied from the second arg with flag bits normalized, sixteen entries of four bytes from the
  third arg starting `[pkt+0xc]`. Only caller: `f_10135f00`.
- `f_100e5420` id `0x51`, commit size `0x48`: `[pkt+4]` = count (guard `cmp bl, 0x10`), entries of four bytes
  starting `[pkt+8]` written from scratch fields byte 4, byte 0, byte 5. Only caller: `f_101a7af0`.
- `f_100e5490` id `0x53`, commit size `0x88`: `[pkt+4]` = count, `[pkt+5] = 3` (Mode = Set), `[pkt+6]` flags
  toggled from a caller argument bit; entries of eight bytes starting `[pkt+8]` written from scratch fields
  byte 4, byte 0, byte 5 and the u16 at 6. Only caller: `f_101a7df0`.
- `f_100e5520` id `0x53`, commit size `0x88`: Mode-only variant, count forced to zero; callers `f_101a7fd0`,
  `f_101a8030`, `f_101a80d0` (twice).

Editor module:

- Row handler `f_10135f00`: gate `f_10135ee0`; cursor word read from `[obj + 0x4c]` through `f_10131680`;
  entry list at `[obj + 0x78]`, stride four; comparisons against byte 1, category (byte 0 shifted right two)
  versus `dword 0x10663740` -> offset `0x60` -> first dword, and item id word at `[obj + 0x7a]`; local mutators
  `f_10136030` and `f_10136050`; item-change staging `f_10136070`. Send sites (callers) in `f_10135190`
  (passes literal `1`), `f_10135650` (twice, service global `dword 0x105781bc`) and `f_10135ab0`; the mode
  values other than that literal were not individually resolved.

LandSandBoat cross-checks (`vendor/server/src/map/`): ids in `enums/packet_c2s.h`
(`GP_CLI_COMMAND_EQUIP_SET 0x050`, `GP_CLI_COMMAND_EQUIPSET_SET 0x051`,
`GP_CLI_COMMAND_EQUIPSET_CHECK 0x052`, `GP_CLI_COMMAND_LOCKSTYLE 0x053`); structs and validators in
`packets/c2s/0x051_equipset_set.{h,cpp}` (count range 1..16, `oneOf<SLOTTYPE>` on each entry's EquipKind,
container validation for Category, and the equip call `charutils::EquipItem(PChar, ItemIndex, EquipKind,
Category)`), `packets/c2s/0x052_equipset_check.{h,cpp}` (`equipsetrequestitem_t` bitfields `HasItemFlg` /
`RemoveItemFlg` / `Category:6`, range check on the packet-level EquipKind 0..15) and
`packets/c2s/0x053_lockstyle.{h,cpp}` (`lockstyleitem_t`, `GP_CLI_COMMAND_LOCKSTYLE_MODE` with Set = 3,
the skip for `EquipKind > SLOT_FEET`, the slot-mask test `(PItem->getEquipSlotId() & (1 << item.EquipKind))`,
`charutils::SetStyleLock`, and the on/off message pair). `SLOT_FEET = 8` in
`entities/battle_entity.h`, so the server's "skip past feet" rule keeps exactly positions 0..8 -
the same nine the client gate admits. Reply surface: `packets/s2c/0x116_equipset_valid.h`.

Unresolved indirect calls, per the lookup procedure: vptr dispatch from the generic menu framework into
`f_101a8190` and its sibling handlers (the vtables above are the only evidence of their entry points);
the virtual method at `[.rdata 0x10333830 + 0x58]` used by arm 0's service; dialog callbacks invoked by the
dialog framework from the literals `f_101a7650` / `f_101a7690` / `f_101a7670`; and the file IO helpers called
inside `f_101a8530`. The jump-table words at `0x101a8394` and the vtable dwords above were read from the
unpacked image's data section before switching to `dis`, so re-verify them with
`tools/newbuild.py dis --at 0x101a8394` if they become load-bearing.
