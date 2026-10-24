# Unity and alter-ego-point windows (Status ids 11 and 13)

Observed 2026-10-15. Scope: clean retail-2026-09 FFXiMain.dll, static reading only. No game DAT, live client or Kuluu runtime was available, so every DAT value below is named by file id / record / text id and was not read. This record covers the windows reached from the Status selector `statcom2` item 11 (`ut_menu`) and item 13 (`fp_cat`) and everything they open.

## Window map

| Window | Role | Opened by | Closed by | Keys / Esc | Placement |
|---|---|---|---|---|---|
| `ut_menu` | Unity menu, 3 live rows | `statcom2` item 11 | Esc | interactive; Esc closes it without any handler action (its close still closes `ut_point`) | own DAT frame |
| `ut_point` | Unity accolades readout | `ut_menu` open | `ut_menu` close | display-only | x from DAT frame; y docked to the party window (below) |
| `quest00` (Unity chat variant) | Unity status panel + chat on/off | `ut_menu` item 1 | its Esc/cancel | interactive; Esc reaches the handler, which closes it | own DAT frame; rows moved (below) |
| `quest00` (ranking chooser variant) | pick a result set | `ut_menu` item 2 | its Esc/cancel | as above | own DAT frame; rows moved (below) |
| `quest01` (ranking variants) | Unity ranking list | ranking chooser row | its Esc/cancel | interactive; Esc closes it and hands keys back to the chooser | own DAT frame |
| `arealist` (Unity leader variant) | Unity concord list feeding the map | `ut_menu` item 3 | Esc, or any row select | interactive; Esc closes it | registry docking slot 0x1000 (log-window dock group), not traced further |
| `fp_cat` | alter-ego-point category list | `statcom2` item 13 | its Esc/cancel | interactive; Esc reaches the handler, which closes it | own DAT frame |
| `fp_cat2` | upgrades of one category | `fp_cat` row select | its Esc/cancel | interactive; as `fp_cat` | own DAT frame |
| `fp_info` | point total readout | `fp_cat` open | `fp_cat` close | display-only | own DAT frame |
| `bluehelp` | description box | `fp_cat` open | `fp_cat` close | display-only | its default placement (no position passed) |
| `merityn` | yes/no confirmation | `fp_cat2` row select | answer / Esc | interactive | own DAT frame |

Every open above uses "activate, not exclusive": the opener stays open underneath and the new window takes focus because it is the topmost non-display-only window. `statcom2` closes its display-only detail companions (profile, joblevel, btlskill, mgcskill, trdskill, masterle) before opening either `ut_menu` or `fp_cat`, and records no open detail, so `statcom2` itself keeps no state for these two. `ut_menu`, `quest00`, `fp_cat`, `fp_cat2` and `arealist` carry the registry bit that closes the macro-palette / `cnttime` windows when they activate (shared selector rule). `ut_menu` also carries the "last DAT item starts unselectable" bit; on `statcom2` the same bit hits a trailing all-blocked dummy block (14 items, 13 live), so the writer should confirm on the `ut_menu` record that live rows are ids 1..3 and the record's last item is a dummy.

## Status selector gates

- Item 11 (`ut_menu`) is never disabled.
- Item 13 (`fp_cat`) is disabled each time `statcom2` opens and becomes selectable once all hold, re-checked every frame and latched until `statcom2` reopens: the Status data is ready (an s2c 0x119 has been received; no reset of this condition was found), main job level >= 99 (s2c 0x061 `CLISTATUS.mjob_lv`, packet offset 0x0D), and key item 3361 is owned (s2c 0x055 `GetItemFlag`, table 6 word 9 bit 1; LSB `scripts/enum/key_item.lua` names 3361 `CIPHER_BRACELET`). It enables one frame after the first ready frame. This matches Kuluu's existing `FacetCategories` gate.

## Unity

### ut_menu

On open, for result set 0 then result set 1: if that set's 32-chunk table has completed since the last c2s 0x116 for it (see "Unity ranking data"), send c2s 0x116 (`GP_CLI_COMMAND_UNITY_MENU.Kind` u32 = 0 or 1, 8-byte packet) and clear that set's complete mark. The ranking views show their not-ready text until the server re-sends chunk 31 (LSB answers c2s 0x116 with s2c 0x110 plus both sets). Then open `ut_point`. Closing `ut_menu` closes `ut_point`. Row labels are the record's own item texts.

| Item | Action |
|---|---|
| 1 | open `quest00` as the Unity chat variant |
| 2 | open `quest00` as the ranking chooser variant |
| 3 | open `arealist` as the Unity leader variant |

No item sends a packet, plays a sound or refuses.

### ut_point

Draws one string in neutral white (color word 0x80808080), right-aligned with its right edge 100 units right of the window's left edge, top 20 units below the window's top edge (window-relative design space; the window is not UI-scaled): `"<points>/99999"` where points = s2c 0x061 `unity_info.Points` (17 bits, LSB fills it from `unity_accolades`), or `"/99999"` before any s2c 0x119 has arrived. Format strings are `"%d/%d"` and `"/%d"`. Static labels come from the record.

Dock rule (registry docking slot 0x2000, the party-window dock group; the same group as `conq3pts`, `imperial`, `allied`, `colopoin`): x stays at the DAT frame. y is set so the window sits directly above the party window: top = partywin.top - height - 2. When the party window's cursor item carries record style 1 or 2, it is bottom-aligned instead: top = partywin.bottom - height. The result is clamped to top >= 16. Re-docking runs when `ut_point` activates and whenever the party window's tracked edge moves. If the party window is not open, the DAT position stands.

### Unity chat variant (`quest00`, from item 1)

Title bar (`titlewin`) shows GetText table 12 id 395 while open and is cleared on close. Rows are DAT items 1 and 2 (items 3 and 4 made unselectable). Each keeps its height and is moved so its bottom edge sits 35 (item 1) / 12 (item 2) units above the window's bottom edge. Row r (item r+1) draws GetText 12 id 440+r at item x+24, plus a marker sprite. The chosen row gets the quest window's check sprite at item (0,1); the other gets the ordinary row bullet at (4,5). Sprite-table indices are in Provenance.

Value model: v = Unity chat mode. On open v = client option 178. Selecting item 1 sets v = 1 (LSB `Active`), item 2 sets v = 0 (`Inactive`). The check marks item 1 when v = 1 and item 2 when v = 0. Every frame, a countdown that starts at 0 on open is decremented. When it is 0 and v differs from the last value sent since this window opened (initially the opening value), the client sends c2s 0x118 (`GP_CLI_COMMAND_UNITY_TOGGLE.Mode` u8 = v, 8-byte packet) and restarts the countdown at 30 frames. On close it stores v into option 178 even if the last change was not sent yet. The client also sends c2s 0x118 with option 178 during zone-in (traced to that call site only). Select plays no sound from the handler. Esc closes.

Panel text, window-relative, color 0x80808080 (labels left-aligned, values ending "pt" right-aligned):
- Always: GetText 12 id 395 at (80,15); GetText 12 id 439 at (20,168).
- Not ready (no s2c 0x119 yet, or either ranking set incomplete): GetText 9 id 97 at (20,43), id 98 at (20,68).
- Ready: labels GetText 12 ids 397, 398, 399, 452, 453 at x=20, y=43, 68, 93, 118, 143. With u = s2c 0x061 `unity_info.Faction`:
  - u = 0: GetText 12 id 402 at (180,43); GetText 12 id 434 at (220,68) and (220,93); nothing else.
  - u != 0: GetText 12 id 402+u at (180,43). Rows 68 / 93 show rank and value of u in result set 0 / 1: rank text at x=220 (rank 0 shows id 434 and no value), value `"%dpt"` capped at 999999 ending at x=350. Row 118 shows `"%dpt"` of s2c 0x061 `unity_points2`, row 143 shows `unity_points1` (LSB: previous / current week accolades / 1000), both ending at x=350.
- Rank and value are computed on the first ready frame after open, then frozen until reopen. The 0x061 values are live.

Rank text: rank 1..3 is GetText 12 id 434+rank. Rank >= 4 uses GetText 12 id 438 as a printf template with the rank.

### Ranking chooser variant (`quest00`, from item 2) and ranking list (`quest01`)

Chooser: `titlewin` = GetText 12 id 396; header GetText 12 id 396 at (80,15). Rows are items 1 and 2 at design-space y 100..118 and 130..148 (items 3/4 unselectable). Row r draws GetText 12 id 400+r at item x+16 with the row bullet at (4,5). Selecting item 1 / 2 hides the chooser's cursor and opens `quest01` showing result set 0 / 1. Esc closes the chooser.

`quest01` open (any use): if an s2c 0x110 has arrived since the last c2s 0x117, send c2s 0x117 (`GP_CLI_COMMAND_UNITY_QUEST.Kind` u32 = 0) first. Ranking view:
- `titlewin` = GetText 12 id 396; header GetText 12 id 400 (set 0) / 401 (set 1) at window (60,10).
- Not ready: GetText 9 ids 97 / 98 at (20,45) / (20,73).
- When ready and the list is empty, it builds once: the 11 Concords (u = 1..11) sorted by value descending (ties: lower u first), 8 rows per page, record items 9/10 page. It is not rebuilt while non-empty.
- Row: row bullet at item (4,5), rank text at item x+16, GetText 12 id 402+u at x+58, `"%dpt"` (cap 999999) at x+250.
- Left/Right switches to the other result set when both sets are complete (rebuilds; cursor sound). Under a paging-key condition (not traced) they page instead.
- Esc closes `quest01` and restores the chooser's cursor.

### Unity leader variant (`arealist`, from item 3)

The variant first resets map marker state (not traced). It lists 12 rows, 5 visible, 16 units apart, columns at x 20 / 245 / 320 / 380.
- Row 0: GetText 12 id 442. Rows 1..11: GetText 12 ids 403..413.
- Column 2: `"(%s)"` with a per-row uppercase ASCII tag. The tags are client-internal constants listed in Provenance, not DAT text.
- Every row's help line: GetText table 0x0E id 892.
- Selecting row i (row 0 means the player's own `unity_info.Faction`) hands that Concord to the map subsystem, closes `arealist` and `scsibori`, and plays the confirm sound. The map side was not traced.
- Esc closes it.

### Unity ranking data (s2c 0x063 type 7)

Each s2c 0x063 packet with type 7 (`vendor/server/src/map/packets/s2c/0x063_miscdata_unity.h`) stores 128 bytes into a per-result-set table:
- Bytes from packet offset 0x0C through 0x8B go into chunk `dataType` (packet offset 9).
- The result set is 1 if `resultSet` (packet offset 8) is nonzero, else 0.
- Receiving chunk 31 marks that set complete and unacknowledged.

The views use chunk 1 (members) and chunk 2 (points). Concord u's value is the u32 at chunk offset 4u, i.e. packet offset 0x0C+4u. value(u) = trunc(points(u) / members(u)), or 0 when members is 0. Ranks are competition ranks (ties share a rank; the next rank skips). If every value is 0, rank is 0.

LSB mismatch: LSB's MEMBERS / POINTS structs put a u16 `statusFlag` at packet offset 0x10 and `members[0]`/`points[0]` at 0x14. The retail client therefore reads LSB's `statusFlag` word as Concord 1 and LSB `members[i]` as Concord i+2.

## Alter ego points ("fp")

What fp is: the Trust (alter ego) upgrade-point system. The client spends points with c2s 0x0C1 (`vendor/server/src/map/packets/c2s/0x0c1_alter_ego_points.h`) and reads them from s2c 0x08E (`vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h`). Supporting facts:
- LSB `char_points.alter_ego_points` holds the balance.
- `src/map/enums/alter_ego_points.h` lists upgrade ids 8..18 (HP, MP, STR..CHR, CombatSkills, MagicSkills).
- LSB's c2s 0x0C1 validator requires Mog House, main job 99 and key item 3361, the same gates the client applies.
- The `statcom2` item 13 description, GetText 0x0E id 1096 (Kuluu's earlier read), reads "Displays Categories for which alter ego points may be spent...".
- No expansion of "fp" exists in the DLL. "Faith points" after Trust's Japanese name is a guess.
- The `fp_cat` caption is that record's frame text A (GetText 0x0E).

DAT sources, resolved through the install's FTABLE like other file ids:
- Category table: file id 98, first 288 bytes = 8 records of 36 bytes: u16 category id, u16 unused here, u16 upgrade ids[16] (0 = empty).
- Upgrade record: file id 97, 4 bytes at byte offset 4 x upgrade id. Byte 3 is the upgrade's level cap.
- Strings: file id 55683 on an English install (55563 Japanese; the language table's entry 0x20). Entry 2n = name, 2n+1 = description, shared by category ids and upgrade ids.

Wire state (s2c 0x08E, offsets are packet offsets):
- `Points` u16 at 4.
- `Upgrades[32]` u8 at 8+id (current level).
- `Costs[32]` u16 at 0x28+2*id.
- Each s2c 0x08E also marks "fp data received" for the session; the mark is cleared at session teardown.

Refusal state "allowed" = in Mog House (s2c 0x00A `LoginState` 1, 4 or 5 at the last zone-in) and not Monstrosity (s2c 0x00A `IsMonstrosity` = 0 and s2c 0x061 `mjob_no` != 23).

### fp_cat

On open:
1. If no s2c 0x08E has arrived this session, send c2s 0x0C2 (4-byte header, no body; LSB has no handler).
2. Load the strings and the category table.
3. Open `fp_info` and `bluehelp`.

On close: release them and close `fp_info` and `bluehelp`.

Rows are built from DAT at open only: records 0..7 in order, listed when category id != 0 and the first upgrade id != 0. Each row draws string 2 x (category id) at column x 6, y+2.

List layout, shared with `fp_cat2`:
- 5 columns at x offsets 0/6/252/253/290 (end 357).
- Rows 17 units apart, the first row 4 units below the window top (`fp_cat2`: 17), 10 visible.
- The cursor row's name is drawn in 0x80803040.
- Standard scrolling list: Up/Down at an edge scroll one row; Left/Right page by 10 under the same untraced paging-key condition.

While `fp_cat` has focus, `bluehelp` shows the highlighted category's description (string 2n+1) with its name (string 2n) as a header line, or blanks with no valid row. Select plays the confirm sound and opens `fp_cat2` for that category id. The id is also used as the record index, so the data must have id = index. Esc closes `fp_cat`.

### fp_cat2

On open:
1. Same c2s 0x0C2 rule as `fp_cat`.
2. Rows build from the selected category's 16 upgrade ids, skipping 0.
3. Every s2c 0x08E that arrives while it is open rebuilds the rows and restores the cursor to the last selected row.

While focused, `bluehelp` shows the highlighted upgrade's description and name.

Row for upgrade id n:
- Name (string 2n) at x 6.
- Current level `Upgrades[n]` right-aligned ending at x 252.
- `"/<cap>"` at x 253.
- Cost cell: GetText 12 id 90 at x 290 and `"<Costs[n]> P"` ending at x 357.

Colors:

| State | Name | Level / cap | Cost cell |
|---|---|---|---|
| not allowed, or no 0x08E yet | 0x80404040 | 0x80404040 | hidden |
| level >= cap | 0x80808080 | 0x802060A0 | hidden |
| affordable (`Points` >= `Costs[n]`) | 0x80808080 | 0x80808080 | 0x80808080 |
| not affordable | 0x80808080 | 0x80808080 | 0x80404040 |

Select, first match wins:
1. No valid row: close `fp_cat2`.
2. Not allowed: client message GetText table 7 id 178 plus buzzer.
3. Level >= cap, no 0x08E yet, or `Points` < `Costs[n]`: buzzer.
4. Otherwise: confirm sound, then open the `merityn` yes/no.
   - Text: GetText table 0x0A id 123 (file 50 English, 32 Japanese) formatted with the upgrade name then the cost.
   - The cursor starts on No (row 2).
   - Yes sends c2s 0x0C1 (`CategoryIndex` u16 = n, pad 0, 8-byte packet). No does nothing.
   - If a prompt is already pending, nothing opens.

Esc closes `fp_cat2`, and focus returns to `fp_cat`, whose description comes back.

### fp_info

Each frame it draws `"<Points>/1000"` right-aligned, ending 8 units left of the window's right edge, 20 units below its top. Color is 0x802060A0 when `Points` >= 1000, else 0x80808080. It reads 0 before any s2c 0x08E. Labels come from the record.

## Refresh and requests

| Trigger | Effect |
|---|---|
| s2c 0x061 | `ut_point` and the Unity panel's 0x061 rows update next frame; the item 13 level gate re-evaluates |
| s2c 0x055 | key item 3361 gate |
| s2c 0x119 (first) | Status ready; `ut_point` and the Unity panel leave their placeholder states |
| s2c 0x063 type 7 | fills ranking chunks; chunk 31 completes a set (feeds the next `ut_menu` c2s 0x116 and the ranking views) |
| s2c 0x110 | arms c2s 0x117 on the next `quest01` open (not displayed here) |
| s2c 0x08E | `fp_info` live; `fp_cat2` rebuilds if open; clears the c2s 0x0C2 need |
| client sends | c2s 0x116 (`ut_menu` open), c2s 0x117 (`quest01` open), c2s 0x118 (chat toggle, throttled; also zone-in), c2s 0x0C2 (`fp_cat`/`fp_cat2` open without data), c2s 0x0C1 (confirmed upgrade) |

## Established vs not established

Established: the windows, opener/closer cascade and focus/Esc behavior above; the Status gates; every packet's trigger, body and size; the DAT file ids, record layouts and string-index rules (by structure, not by content); all GetText ids and their positions; the dock rule; refusal order, sounds and message id; the LSB 0x063 type-7 one-slot offset.

Not established:
- Any DAT text or value. That includes whether category records really have id = index, the actual caps, and the label texts.
- Which party-window cursor style 1/2 means for docking, and the docked window's own stored lower bound.
- The paging-key condition for Left/Right.
- `arealist`'s map handoff and its own docking for this variant.
- The sprite atlases behind the two marker sprites.
- Whether the session teardown that clears the fp and ranking flags happens on every zone change.
- The `merityn` Esc result.
- The format engine behind GetText 0x0A id 123 (argument order only).

## Provenance

All addresses are RVAs in retail-2026-09 (image base 0x10000000; raw offset == RVA in `/home/claude/ffximain/FFXiMain.unpacked.dll`). Static capstone reading only; no process run.

Registry (entry RVA, flags u32): ut_menu 0x37445C 0x00220000; ut_point 0x374488 0x00002001; fp_cat 0x3744B4 0x00030002; fp_cat2 0x3744E0 0x00030002; fp_info 0x37450C 0x00000001; quest00 0x374D20 0x00030002; quest01 0x374D4C 0x0001000A; arealist 0x373AE8 0x00031002; bluehelp 0x373D24 0x00000003; merityn 0x3743AC 0x00031002; partywin 0x373250 (dock group 1 anchor). Activation docking RVA 0x15E5D1..0x15E650, group masks table RVA 0x37629C {0x1000,0x2000,0x4000,0x10000000}, anchors set at RVA 0x15D952 (logwindo, partywin, targetwi, query); dock routine RVA 0x15F790 (style test via cursor item byte at item offset 0x44; minus-2 gap at RVA 0x15F88F; clamp 16 at RVA 0x15F8A8); per-frame re-dock pass RVA 0x15FCAB..0x15FD9E (anchor-moved test RVA 0x15F8F0). 0x200000 rule RVA 0x1190F0 (state 0x10 on item id == count), applied in OpenMenu at RVA 0x15E4B0. Activation closing 0x40000 windows RVA 0x15E694. Refresh call with focus flag RVA 0x15FDF0..0x15FE05.

statcom2: select RVA 0x148D00 (id 11 -> OpenMenu string RVA 0x371964; id 13 -> RVA 0x371950; detail close RVA 0x148AC0); open RVA 0x148E80 / disabler RVA 0x148B50 (state 5 on ids 3,4,5,12,13); refresh RVA 0x148BE0 (ready test RVA 0x146310 -> persona byte set at RVA 0x147091 when status flag bit 0 set; item 13 at RVA 0x148CC8..0x148CEE: level byte VA 0x10485641 >= 0x63, key item 0xD21 via RVA 0x97E80/0x97E50 on table VA 0x1047FEB8 filled by s2c 0x055 handler RVA 0x98710). Item state setter RVA 0x118B60 takes (state, id); state switch RVA 0x119970 (0xB normal, 5 grey, 0x10 unselectable).

Status flag byte VA 0x10485824: bit 0 set only in s2c 0x119 handler RVA 0x99430 (or at RVA 0x9948A). s2c 0x061 handler RVA 0x994A0: packet 0xD -> VA 0x10485641, packet 0x58 dword -> VA 0x1048568C (Faction getter RVA 0xA3610, Points getter RVA 0xA3630), packet 0x5C dword -> VA 0x10485690 (low/high getters RVA 0xA3660/0xA3670).

ut_menu vtable RVA 0x334580: open RVA 0x14C260 (pending-send helper RVA 0x996A0 with flags VA 0x10483160/0x10483162; c2s 0x116 builder RVA 0xFD5E0), close RVA 0x14C290, input RVA 0x14C2A0 (quest00 mode word at handler offset 0x68 = 1 / 2 via global VA 0x10668ECC, open string RVA 0x3580E0; item 3 -> arealist method RVA 0x20F3A0 on VA 0x106644C0). ut_point vtable RVA 0x335E08: per-frame RVA 0x14C340 (formats RVA 0x3719D4 "/%d", RVA 0x357FBC "%d/%d"; right-aligned text RVA 0x1C9B50; window rect getter RVA 0x118F70 returns left,top,right,bottom).

s2c 0x063 handler RVA 0x99270, type 7 arm RVA 0x99351..0x9939B (copy 0x20 dwords from packet 0xC into VA 0x10483164 + (set<<12) + (dataType<<7); complete when dataType*32+32 >= 0x400). Reset at RVA 0x939EC. Ranking math: decode RVA 0xFDE80 (field reader RVA 0xFDDF0, kinds 1/2), sort compare RVA 0xFDE10, rank RVA 0xFDF20, value RVA 0xFDF80, all-zero test RVA 0xFDE50, wrapper RVA 0xFDFB0. Rank text RVA 0xA37A0 (ids 0x1B2+r, template 0x1B6). Unity names RVA 0xA36F0 (0x192+u; alt 0x1A2+u unused here), row-0 variant RVA 0xA36C0 (0x1BA).

quest00 handler vtable RVA 0x33C6B0 (ctor RVA 0x22F610): mode dispatch build RVA 0x22FB10 (mode 1 RVA 0x22F8A0, mode 2 RVA 0x22FA10), per-frame RVA 0x2300A0 (mode 1 panel RVA 0x22FB50, mode 2 RVA 0x230060), input RVA 0x230230 (mode 1 RVA 0x230150: value = id & 1 at offset 0x6E; mode 2 RVA 0x2301B0 -> quest01 mode 5/6 via RVA 0x230740), refresh RVA 0x2302E0 (mode 1 throttle RVA 0x2302B0/0x230310, countdown 0x1E, c2s 0x118 builder RVA 0xFD680), close RVA 0x230370 (option store write RVA 0x193830 id 0xB2; titlewin clear via VA 0x1057847C method RVA 0x150CC0). Row callbacks RVA 0x22F7E0 (mode 1; sprites via RVA 0x220CF0: set 4 index 0xAA at (0,1) when row != value, set 0 index 0x23 at (4,5) otherwise) and RVA 0x22F850 (mode 2). Zone-in c2s 0x118 from option 0xB2: RVA 0x95610 called via RVA 0x95650 from RVA 0x952CC.

quest01 handler vtable RVA 0x33C648: open RVA 0x2326A0 (c2s 0x117 gate RVA 0x871A0 on flag VA 0x104807EC bit 0, set by s2c 0x110 handler RVA 0x99070; builder RVA 0xFD650), refresh RVA 0x2326B0 -> RVA 0x231A90/0x2319C0 (page size 8, capacity 11, sorted buffer VA 0x106689E0), per-frame RVA 0x232720 -> RVA 0x231930 (header RVA 0x2317A0), row RVA 0x231840, input RVA 0x233090 -> RVA 0x231AC0 (set toggle RVA 0x231B87; cancel RVA 0x231B1C).

arealist vtable RVA 0x33BA68: Unity mode setup RVA 0x20F3A0 (mode byte 4), rows RVA 0x20F290 (column table RVA 0x385A18; tag table RVA 0x364880 -> "UNITY","PIEUJE","AYAME","ISHIELD","APURURU","MAAT","ALDO","JAKOH","NAJA","FLAVIRIA","YORAN","SYLVIE"; "(%s)" RVA 0x382120, cell type 0x21 RVA 0x1F5836; help GetText 0x0E 0x37C), input hook RVA 0x20EF00 (select RVA 0x20F440 -> map object VA 0x10664494 method RVA 0x20A9D0, CloseMenu scsibori RVA 0x385A64), close slot RVA 0x20EE60.

fp classes: generic list base vtable RVA 0x33A730 (ctor RVA 0x1F67C0, open RVA 0x1F68B0, close RVA 0x1F6980, refresh RVA 0x1F69D0, per-frame RVA 0x1F6A60, input RVA 0x1F6C70 with code table RVA 0x1F6FF8, cursor index RVA 0x1F7100, row cell setter RVA 0x1F4B20, cell draw RVA 0x1F4BB0 types 0x49/0x4A/0x4B/0x4C at RVA 0x1F641E/0x1F6471/0x1F64C3/0x1F6509; column table RVA 0x371870). fp_cat vtable RVA 0x334B98 (ctor RVA 0x14B670; populate RVA 0x14B750, close RVA 0x14B7D0, refresh RVA 0x14B800, input RVA 0x14B860, rows RVA 0x14B950). fp_cat2 vtable RVA 0x334C08 (ctor RVA 0x14BA60; set-category-and-open RVA 0x14BB50; populate RVA 0x14BB70 with s2c-0x08E hook RVA 0x14BBD0 stored at session offset 0x2D678; close RVA 0x14BBE0; refresh RVA 0x14BC00; input RVA 0x14BC70; rows RVA 0x14BE40; maxed RVA 0x14C020; affordable RVA 0x14C070; prompt RVA 0x14C0F0 with formatter RVA 0x153180 (table 0xA id 0x7B) and yes/no service RVA 0x210E30 on merityn VA 0x10578438, default row 2 at RVA 0x210ECB; YES callback RVA 0x14C180 -> c2s 0x0C1 builder RVA 0xE50D0). Refusal message RVA 0x129E30 (GetText table 7) id 0xB2; buzzer RVA 0x37C50; confirm RVA 0x37B90; cursor RVA 0x37B30. Allowed predicate RVA 0x1CC070 (state word bit 0x100 = 'R', set for LoginState 1/4/5 at RVA 0xFADBA..0xFAE15) and Monstrosity RVA 0x97D50 (session word from s2c 0x00A packet 0x7E at RVA 0xFB45C, or job 0x17). fp_info vtable RVA 0x334538 (per-frame RVA 0x14C1B0, cap 0x3E8, colors 0x802060A0/0x80808080). bluehelp setup RVA 0x1EC220 (-999,-999), text RVA 0x1EC330, blank RVA 0x1EC3C0, close RVA 0x1EC2D0.

fp data object VA 0x10578448 (vtable RVA 0x335388; record cache base RVA 0x1C7F80/0x1C8020/0x1C7ED0): begin-view RVA 0x161670 (string table via language index 0x20 RVA 0x25B560 -> tables RVA 0x38A548 (EN 55683) / 0x38AB08 (JP 55563); category read RVA 0x161630 = file 0x62, 0x120 bytes; record file 0x61 set at offset 0x20, record size 4 RVA 0x161800, offset id*4 RVA 0x1617F0); strings 2n / 2n+1 RVA 0x161720 / 0x161740; category record RVA 0x161770; s2c 0x08E block RVA 0x1617A0 (session offset 0x2D614), costs RVA 0x1617B0 (offset 0x2D638). s2c 0x08E handler RVA 0xE5140 (copies 9 dwords from packet 4 and 16 dwords from packet 0x28, calls hook, sets received bit 0x10 at session offset 0x40DB5); received test RVA 0x14B630; c2s 0x0C2 builder RVA 0xE5110; resets RVA 0xE4385/0xE43F0 (teardown caller RVA 0xF9E8A). GetText table map pair list RVA 0x371DA8 (table 7 -> EN 55646, 9 -> 55648, 0x0C -> 55653, 0x0E -> 55651); table 0x0A object never loaded (refs RVA 0x152901/0x152921 only), fallback registration RVA 0x153B53 (EN file 0x32) / RVA 0x153B13 (JP 0x20).

Own throwaway notes: none kept outside the scratchpad. No source edits, commits or runtime actions occurred.
