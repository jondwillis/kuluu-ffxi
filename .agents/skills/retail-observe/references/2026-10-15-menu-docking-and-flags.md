# Menu docking law and registry flag behaviors

Observed 2026-10-15. Scope: clean retail-2026-09, static binary reading only. No retail session or Kuluu runtime was driven and no game DATs were available, so window record values are named, never quoted. This record states where docked menu windows sit, when they move, and what the other registry flags do on screen. It builds on the window-stacking record of the same date (record frames, frame anchor byte, draw order, focus law).

## Dock slots

Four bits of a window's registry flags word put it in a dock slot. The slot names the anchor window it sits against.

| flag | anchor window | fallback |
| --- | --- | --- |
| 0x1000 | chat log window `logwindo`. While log window 2 (`logwin2`) is open, `logwin2` instead, unless a client configuration option (not identified) keeps it on `logwindo` | none |
| 0x2000 | party window `partywin` | none |
| 0x4000 | target window `targetwi` | `partywin`, whenever the target placement does not happen |
| 0x1000_0000 (bit 28) | event question window `query` | none |

No registry entry carries more than one slot bit, and none carries bit 28 (see "Item card" for its only user). Anchors can be docked themselves: `targetwi` carries 0x2000 and `query` carries 0x1000, so chains form (party window, then target window, then 0x4000 windows; log window, then question window, then the item card). The party window positions itself from the log windows' edges by its own logic, which this record does not cover.

## Placement law

Coordinates are menu coordinates: the space a window's record frame lands in after its frame anchor adjustment (window-stacking record). For anchor A and docked window W:

- x is unchanged. W keeps its current left edge, which is its record frame x after the anchor adjustment unless W's own logic moved it.
- Anchor frame shown or fading in: W.top = A.top - W.height - 2. W's bottom edge sits 2 pixels above A's top edge.
- Anchor frame hidden or fading out: W.top = A.bottom - W.height. W's bottom edge is level with A's bottom edge, so W takes A's place.
- Then, if W has a lower bound (only the item card sets one), W.top = max(W.top, bound).
- Then W.top = max(W.top, 16). A docked window never sits above y 16.
- W keeps its size; its items move with it.

"Frame hidden" means A's window background has been faded out by A's own logic (the chat log fades its background when it has nothing to show). A fade runs 20 client frame ticks each way; while fading out A already counts as hidden, while fading in it already counts as shown.

Nothing is placed, and W keeps whatever position it has, when A is not open, when A is W itself, or while docking is suspended (below).

## When docking runs

1. Activation. When W becomes active (opened with activation, or a deferred activation), it is placed once against its slot's anchor. A 0x4000 window tries the target window and falls back to the party window when that placement did not happen. With no anchor available W stays at its record frame.
2. Every frame, in the in-zone play state only. Slots are visited in the order log, party, target, question, log 2. For each, if the anchor's watched edge differs from the value remembered for that slot, or the anchor is not open, every open window that carries the slot bit and is not closing is placed again. The watched edge is the anchor's top edge (its bottom edge if the anchor window is set to draw without its frame); the new value is remembered. The 0x4000 fallback applies here too, so while no target window is open the 0x4000 windows follow the party window every frame. Because party is visited before target and log before question, each chain settles within one frame.
3. On request from a window's own logic. A request forgets the remembered edge of one slot (so the next frame's pass re-places that slot's windows) and places one window immediately. Known requesters: the Command menu (`playermo`) when it refreshes or switches between its `actionm1`..`actionm8` layout records; the target window when it opens (forget only, since a window never docks against itself); the item card after it opens or changes situation.

Consequences a writer must keep:

- An anchor that closes leaves its docked windows where they are. They do not return to their record frames.
- A frame fade alone does not move docked windows. A window opened while the log's background is faded docks level with the log's bottom and stays there after the background fades back in, until the log's top edge moves or a request comes.
- A docked window that its own logic moves after docking keeps that position until the next trigger.
- Re-placing every frame from the law above gives the same picture except in the last two cases.

## Several windows in one slot

There is no stacking within a slot. Each window carrying a slot bit is placed against the anchor alone, so windows that share a slot share one bottom line (2 pixels above the anchor's top, or level with its bottom) and overlap. Which one covers the other is ordinary draw order (newest on top within its priority). Example: `targetwi`, `ut_point` and the four point readouts all sit on the same line above the party window.

## Suspension

While an event script has put the event message window (`rem4line`) into its scripted layout and the client's event-time interface predicate holds, no docking happens at all: activation, the frame pass and requests all leave windows where they are. The predicate is shared by many interface paths and was not tied to a user-visible condition here.

## Registry windows with dock flags

Every registry window that carries a slot, from the whole registry (its flag words are under Provenance). The party-slot and target-slot holders are exactly `targetwi`, `ut_point`, the four point readouts and `subwindo`. Identities are given only where an earlier record or the window logic read for this record supports them; blank means not identified.

| window | slot | identity |
| --- | --- | --- |
| `query` | log | event question window (opened by event instructions) |
| `gmtell` | log | |
| `playermo` | log | Command menu (Attack and the target command rows; `actionm1`..`actionm8` layouts) |
| `mp_pmode` | log | |
| `chatctrl` | log | |
| `magselec` | log | |
| `abiselec` | log | |
| `mp_abisc` | log | |
| `myroom` | log | Mog House menu |
| `rmpost` | log | |
| `mogdoor` | log | |
| `mogext` | log | |
| `mogpost` | log | Mog House Delivery Box window |
| `roomlist` | log | Mog House Remodel list |
| `storage`, `storage2` | log | |
| `jobchang` | log | Mog House Change Jobs |
| `jobcsel`, `jobcselu` | log | |
| `arealist` | log | Unity concord list variant (Unity record) |
| `shopmain` | log | shop Buy/Sell picker |
| `guildsho` | log | |
| `merityn` | log | merit Yes/No (merit record) |
| `maplist` | log | |
| `comyn` | log | generic Yes/No confirm (Log Out, Shut Down, Dismount, Remodel, key-assignment reset) |
| `resyn`, `resynus`, `resynde`, `resynfr` | log | |
| `magic` | log | spell list |
| `ability` | log | ability list |
| `emote` | log | emote list |
| `mount` | log | mount list |
| `rmlo1`, `rmlo2`, `rmlo4` | log | Mog House arrange command windows (arrange record) |
| `plnt1`..`plnt6` | log | |
| `targetwi` | party | target window (display-only) |
| `ut_point` | party | Unity accolades readout (display-only) |
| `conq3pts`, `imperial`, `allied`, `colopoin` | party | display-only point readouts |
| `subwindo` | target | display-only; opened by the spell list and `chatctrl` |

## Item card

The item card `iteminfo` (display-only, Esc goes to its own logic, no slot of its own in the registry) gains a slot bit at run time, right after it opens, in two situations:

| situation | slot | lower bound |
| --- | --- | --- |
| shown by the event instruction that displays an item (operand: item id; zero hides the card) | log (0x1000) | y 240 |
| shown together with an event question | question (bit 28) | y 16 |

The lower bound is the card's own preset y for that situation, passed through the same code-move helper as other code moves (window-stacking record). Every open re-copies the registry flags, so the bit lasts only until the card is opened again. In its other situations the card sits at fixed preset positions and does not dock; those are outside this record.

## Registry flag behaviors

| flag | behavior |
| --- | --- |
| 0x1 | Display-only: the focus law skips it, so it never takes the keys. Code can explicitly hand such a window the keys (window-stacking record: `joblevel` in job-change mode). |
| 0x2 | Esc reaches the window's own logic as a cancel, and that logic decides what closes. Without it, Esc closes that one window and its logic never sees the key. The cancel sound plays in both cases. |
| 0x8 | Correction to earlier notes, which read this as "opens without a cursor": the window opens and closes without the open/close pop animation, appearing and disappearing at once. Without the flag the pop animation runs 25 client frame ticks, or 0 when a client option (not identified) turns window animation off. Cursor, highlight and focus are unaffected. Holders: `inline`, `fep`, `feppalle`, the four macro palettes, `cnttime`, `quest01`, `evitem01`, `guide01`, and lobby and debug windows. |
| 0x10000 | Every open call puts the cursor on DAT item id 1. Without it the cursor goes to the item remembered for that window: the cursor item when the window last closed (nothing is remembered when it closed on an unselectable item), item 1 if nothing was ever remembered, clamped to the record's item count. This also happens when the window is opened again while already open: the cursor jumps to item 1 or to the remembered item, and the current position is not kept. The memory lasts the session. A window's own logic may move the cursor right after opening (see "Yes/No confirms"). |
| 0x20000, 0x40000 | Activating a 0x20000 window closes every open 0x40000 window (the four macro palettes `mcr1pall`, `mcr2pall`, `mcr1long`, `mcr2long`, and `cnttime`). Not symmetric: activating a 0x40000 window closes nothing. The open check additionally consults 0x40000 against open 0x20000 windows in one in-zone input state (not established, below). |
| 0x100000 | The window's rectangle is scaled by (screen width / 640, screen height / 480). Only the lobby windows `ptcbgwin` and `hnbackwi` carry it; neither docks nor anchors. |
| 0x200000 | When the window instance is created (not when an already open window is opened again), the item whose DAT item id equals the record's item count becomes unselectable. A direction key that would move the cursor onto it leaves the cursor where it is (the window's logic still receives the direction); the pointer ignores it; it is drawn in the normal colour, not greyed. If the open-time cursor placement lands on it (only when item 1 is the last item, or code placed the cursor there) it becomes an ordinary highlighted item. While the cursor rests on an unselectable item the window ignores every key, Esc included. Order at creation: this rule, then the open-time cursor placement, then the window's own open logic, which may set any item's state again (for example the Status selector's enable and disable pass). |

## Yes/No confirms

One shared confirm routine drives the client's Yes/No prompts. It opens its window (activated, opener left open), then moves the cursor to item 2 (No) unless the caller asked for a Yes default; item 1 is Yes. The windows it drives: `comyn` (Log Out, Shut Down, Dismount, Mog House Remodel, key-assignment reset and most other prompts), `merityn`, `resyn`, and `shopbuy` / `shopsell` (the shop's priced buy and sell confirms). The call sites read for this record (Log Out, Shut Down, Dismount, Remodel, key-assignment reset, the shop's buy and sell confirms) all pass the No default; at least one other caller asks for Yes, so the default is per prompt. `comyn`, `merityn` and `resyn` dock above the log window; `shopbuy` and `shopsell` have no dock bit and sit at their record frames.

## Cursor at open

Each window below applies one rule every time it is opened, including an open call on a window that is already open:

- item 1: the cursor goes to DAT item id 1.
- remembered: the cursor goes to the item that held it when this window last closed (nothing is remembered when it closed on an unselectable item); item 1 if nothing is remembered; clamped to the record's item count. The memory is per window and lasts the session.
- display-only: the window never takes the keys, so its cursor never matters.

A window's own logic may move the cursor after the rule. The shared Yes/No confirm routine always does so for the windows it drives (`comyn`, `resyn` and its variants, `merityn`, `shopbuy`, `shopsell`): item 2 (No) unless the prompt asks for Yes. Event scripts may move the `query` cursor (not traced). The grouping is only for lookup; the rule belongs to the window name.

| group | item 1 | remembered | display-only |
| --- | --- | --- | --- |
| Main menu pages | | `menuwind`, `socialme` | |
| Status selector and details | | `statcom`, `statcom2` | `persona`, `level`, `joblevel`, `profile`, `btlskill`, `mgcskill`, `trdskill`, `masterle` |
| Key item list (`/keyitem`; also a Status detail) | `evitem`, `evitem01` | | |
| Merit and job points | `meritcat`, `merit3`, `merit2ca`, `jbpcat` | `merit1`, `merit2` | `levmerit`, `meritinf`, `levmeri2` |
| Unity and alter-ego points | `arealist`, `fp_cat`, `fp_cat2` | `ut_menu` | `ut_point`, `fp_info`, `bluehelp` |
| Command menu and its pickers | `playermo` | `magselec`, `abiselec`, `chatctrl` | `subwindo` |
| Magic, abilities, emotes, mounts | | `mgcmenu`, `magic`, `mgcsortw`, `blusortw`, `abimenu`, `ability`, `abisortw`, `emote`, `mount` | |
| `mp_` windows | `mp_dead`, `mp_pmode` | `mp_abisc`, `mp_mnabi`, `mp_mncst`, `mp_invnt`, `mp_mnnm`, `mp_cstm`, `mp_mntp2`, `mp_kncst` | `mp_stat` |
| Items and equipment | `inventor`, `iuse` | `itemctrl`, `itmsortw`, `itmsort2`, `itmstora`, `itemxinf`, `equip`, `inspect`, `pupequip`, `automato`, `pupsibor` | `iteminfo`, `itemyinf`, `statpup` |
| Trade, bazaar, delivery, loot | `trade`, `gift`, `handover`, `delivery`, `post1`, `post2`, `stringdl`, `bank`, `loot`, `lootope` | `moneyctr`, `bazaar`, `comment`, `lnowin` | `money` |
| Party | `levelsyn` | `prty1`, `prty2`, `prty3`, `prty4`, `prty5`, `prty5us`, `prty5fr`, `prty5de`, `prty7`, `prty8`, `prtyjoin`, `alarm` | |
| Search | `scresult`, `scoption`, `acoption` | `searchma`, `scsibori`, `searchjo`, `scchar`, `searchle`, `searchna`, `searchra`, `scname`, `comgenre`, `sccompar`, `sccomite`, `sccomls`, `scmlev` | |
| Linkshell | `link1`..`link5`, `link7`..`link13` | `lscswin` | `link6` |
| Quests, missions, guide | `quest00`, `quest01`, `miss00`, `guide00`, `guide01` | `missionm` | |
| Region and map | `scanlist`, `maplist` | `region`, `map0`, `mapframe`, `mapv2`, `mapv3`, `mapscan` | |
| Auction House | `auclist`, `auchisto` | `auc1`, `auc2`, `auc3`, `auc4`, `aucweapo`, `aucammo`, `aucarmor`, `aucmagic`, `aucmater`, `aucfood`, `aucmeals`, `aucitem`, `wepsort`, `armsort` | |
| Macros | `mcrselop`, `mcresmn`, `mcrbedit`, `mcrledit` | `mcrmenu`, `mcrselec`, `mcres20`, `mcresed`, `macro`, `mcr1pall`, `mcr2pall`, `mcr1long`, `mcr2long`, `mcr1edit`, `mcr2edit`, `mcr1edlo`, `mcr2edlo`, `mcrmogc` | |
| Config hub, pages, sub-windows | `confyn`, `textcol1`, `conf11l`, `conf11s` | `configwi`, `conf1win`, `conf2win`, `conf3win`, `conf4`, `conf5m`, `conf5win`, `conf5w1`, `conf5w2`, `conf6win`, `conf7`, `conf11m`, `conf12wi`, `conf13wi`, `cfilter`, `conftxtc`, `fxfilter`, `k1assign`, `k2assign`, `textcol2`, `textcol3` | `keylayou` |
| Communication | `friend`, `msglist`, `blklist` | `commenu`, `flistmai`, `flmes`, `olstat`, `oplev`, `msgline`, `mes1rcv`, `mes2frnd`, `blkmain` | |
| Help desk | | `faqsub`, `faqmain` | |
| Mog House | `mogdoor`, `roomlist` | `mogcont`, `myroom`, `rmpost`, `mogext`, `mogpost`, `bankmenu`, `storage`, `storage2`, `mnstorag`, `jobchang`, `jobcsel`, `jobcselu`, `rmlo1`, `rmlo2`, `rmlo4`, `plnt1`..`plnt6` | |
| Synthesis | `cmbhlst` | `cmbmenu`, `cmbhwin` | |
| Shop | `shop` | `shopmain`, `shopbuy`, `shopsell`, `guildsho` | |
| Yes/No prompts | `comyn`, `resyn`, `resynus`, `resynde`, `resynfr`, `merityn`, `confyn` | `sortyn` | |
| Blue magic sets | | `bluinven`, `bluequip`, `bluesibo` | `bluepoin` (and `bluehelp`, above) |
| Event, target and log windows | `query`, `gmtell` | `rem4line`, `rem4li2`, `fulllog`, `charlnk` | `targetwi`, `partywin`, `logwindo`, `logwin2`, `spoolmsg`, `splmsg2` |

## Last item unselectable at creation

These registry windows, and no others, make the item whose DAT item id equals their own record's item count unselectable when the window instance is created; the behavior is in the flag table above (cursor and pointer cannot take it, it is drawn in the normal colour, the window's own open logic runs afterwards and may change it). Where that last item is a trailing dummy block, as on the Status selector (Unity record), nothing visible changes.

| group | windows |
| --- | --- |
| Status selector | `statcom`, `statcom2` |
| Unity | `ut_menu` |
| Magic and abilities | `mgcmenu`, `abimenu`, `abiselec` |
| `mp_` windows | `mp_abisc` |
| Items | `iuse` |
| Party | `prty1`, `prty2`, `prty3`, `prty4`, `prty5`, `prty5us`, `prty5fr`, `prty5de`, `prty7`, `prty8` |
| Linkshell | `link1`, `link2`, `link3`, `link4`, `link7`, `link8`, `link9`, `link10`, `link13` |
| Search | `searchma`, `scoption`, `acoption`, `scsibori`, `searchjo`, `searchle`, `searchna`, `searchra`, `comgenre`, `sccompar`, `sccomite`, `sccomls` |
| Communication | `commenu`, `flistmai`, `mes1rcv`, `mes2frnd`, `blkmain` |
| Map | `map0`, `mapv2`, `mapv3`, `mapscan` |
| Auction House | `auc1`, `auc2`, `auc3`, `auc4`, `aucweapo`, `aucammo`, `aucarmor`, `aucmagic`, `aucmater`, `aucfood`, `aucmeals`, `aucitem`, `wepsort`, `armsort` |
| Macros | `mcrmenu`, `mcrselec`, `mcrselop`, `mcres20`, `mcresmn` |
| Config | `configwi`, `conf5m`, `conf11m`, `conf11l` |
| Yes/No prompts | `sortyn` |
| Blue magic and automaton | `bluesibo`, `statpup` |

## Established and not established

Established: the slot-to-anchor mapping including the log window 2 substitution and the target-to-party fallback; the placement law (2-pixel gap, bottom-level placement while the anchor frame is hidden or fading out, lower bound, floor y 16, x unchanged); the three triggers, the per-frame slot order and its in-zone gate; absent-anchor behavior; no stacking within a slot; the dock-flag holders; the item card's two docked situations; the flag behaviors in the table above, including the 0x8 correction; the per-window cursor-at-open and last-item tables, read from the whole registry.

Not established:

- which client configuration option keeps 0x1000 windows on log window 1 while log window 2 is open;
- the user-visible conditions behind the suspension;
- the party window's own placement from the log window edges;
- the 0x40000 open check: as read, in one player input state a 0x40000 window may open while a 0x20000 window is open only if the newest-first scan finds that 0x20000 window last; the state itself was not identified;
- whether the code-move helper offset changes the item card's lower bounds;
- the meaning of flag bits not covered here (0x10, 0x80, 0x200, 0x400, 0x800, 0x8000, 0x80000, 0x400000, 0x800000, 0x01000000, 0x02000000, 0x08000000, 0x20000000);
- the DAT item counts of the 0x200000 windows, so which visible row (if any) each loses;
- which item card situation the Items window uses.

## Provenance

All code locations are RVAs in retail-2026-09 (TDS 0x6A995428, image base 0x10000000), read from `/home/claude/ffximain/FFXiMain.unpacked.dll` (raw offset == RVA). Static reading only.

Registry extent: 370 entries of 0x2C bytes at RVA 0x3722D8..0x376244. Both lookup loops (bind RVA 0x15E950, open check RVA 0x15E0F0) walk until an entry whose first name byte is zero (RVA 0x376270). The entry at RVA 0x375B0C is named `tkdebug dbdelsel`, so a dump that stops at the first name not starting with `menu` (scratch `dis/menu_table.txt`, 327 entries) misses the 42 entries after it: lobby windows, `chdummy`, the `mp_` windows, `mnstorag`, `mcrmogc`, `cmbmenu`, `cmbhlst`, `cmbhwin`. Full dump in scratch `reader_kit/dock/fullreg.txt`; the per-window tables come from `reader_kit/dock/tables.py`: item 1 = flag 0x10000 without 0x1, remembered = neither 0x10000 nor 0x1, display-only = 0x1, last-item table = flag 0x200000 (74 windows). The key item identity of `evitem`: command table at RVA 0x357170 (id, handler) pairs map command id 0x4C to RVA 0x7D220, which name-opens `evitem` (string RVA 0x3580CC); the next pair, id 0x4D, is the `quest00` opener RVA 0x7D240; Status detail table RVA 0x3716B8 entry 5 is also `evitem`.

Flag words of the docked windows: `query`, `gmtell` 0x00011012; `playermo` 0x08011C02; `mp_pmode` 0x04011C02; `chatctrl` 0x00021202; `magselec` 0x20001A00; `abiselec` 0x20201200; `mp_abisc` 0x00221200; `myroom`, `mogext`, `mogpost` 0x00001002; `rmpost`, `storage`, `storage2` 0x00001000; `mogdoor`, `roomlist`, `arealist`, `merityn`, `maplist`, `comyn` 0x00031002; `jobchang`, `shopmain`, `guildsho`, `plnt1`..`plnt6` 0x00021002; `jobcsel`, `jobcselu`, `rmlo1`, `rmlo2`, `rmlo4` 0x00021000; `resyn` family 0x00011002; `magic`, `ability`, `emote`, `mount` 0x20001002; `targetwi` 0x02002401; `ut_point`, `conq3pts`, `imperial`, `allied`, `colopoin` 0x00002001; `subwindo` 0x00004001. Item card `iteminfo` 0x01400003.

Anchor table: manager init RVA 0x15D900 writes manager +0x8C..+0x9C = handler globals VA 0x1062193C (`logwindo`), 0x106643A8 (`partywin`), 0x10578478 (`targetwi`), 0x1057845C (`query`), 0x10621940 (`logwin2`) at RVA 0x15D952..0x15D97A; remembered edges manager +0xA0..+0xA8 zeroed at RVA 0x15D9A0. Slot resolver RVA 0x15F960 calls RVA 0x15F980: slot 0 becomes 4 when the `logwin2` handler has a window, unless config getter RVA 0x193850(0xB4) returns 2. Edge accessors RVA 0x15F9C0 / 0x15F9E0. Mask table RVA 0x37629C = {0x1000, 0x2000, 0x4000, 0x10000000, 0}.

Dock routine RVA 0x15F790(anchor global, window): anchor null or no window, or anchor window == window, returns 0 (RVA 0x15F79E..0x15F7F1). Suspension RVA 0x15F7CF..0x15F7E8: `rem4line` handler VA 0x105781CC byte +0x48 set and predicate RVA 0x87070(0) true. Rects via RVA 0x118F70 (window +0x3A..+0x40 = left, top, right, bottom). Frame part = first part with type word 0 (RVA 0x118220); its state byte +0x44 equal to 1 or 2 selects the bottom-level branch RVA 0x15F869..0x15F87C, else RVA 0x15F87E..0x15F88F (minus 2 at RVA 0x15F88F). Lower bound RVA 0x15F893..0x15F8A6: window word +0xBE applies when it differs from +0xC2. Floor 16 at RVA 0x15F8A8. MoveTo RVA 0x119110 with the window's current left. The attached top-right marker (window +0x88) is set to (right minus 8, top) at RVA 0x15F8BB..0x15F8D5.

Frame states (frame part +0x44): 0 shown, 1 hidden, 2 fading out, 3 fading in; timer +0x48 = 0x14, update RVA 0x119707..0x119799 (2 to 1, 3 to 0, alpha from timer times 0.05). Manager fade-in / fade-out by name RVA 0x15EB80 / 0x15EBD0; log class fade RVA 0x162C80..0x162CB0; `spoolmsg` refresh fade RVA 0x137412..0x137437; flag 0x8000 auto fade RVA 0x15FF29..0x15FF77.

Activation RVA 0x15E530: dock block RVA 0x15E5D1..0x15E653 (0x1000 slot 0; 0x4000 slot 2 then slot 1 at RVA 0x15E5EB..0x15E616; 0x2000 slot 1; 0x10000000 at RVA 0x15E635 passes the mask itself as the slot index, which would index far past the anchor table; unreachable because RVA 0x15E9C9 re-copies registry flags at every open and no registry entry carries the bit, while the item card adds it only after opening).

Per-frame pass in manager update RVA 0x15FA00: gate game state global VA 0x10456A28 == 0x60 at RVA 0x15FC94..0x15FCA3; loop RVA 0x15FCAB..0x15FD9E over slots 0..4; edge test RVA 0x15F8F0 (window byte +0x69 nonzero selects top, else bottom; absent anchor returns changed without storing); per-window iteration RVA 0x15EE00 skips windows with byte +0x70; mask test RVA 0x15FD58; slot 2 fallback RVA 0x15FD7B..0x15FD8D. Slot 0 change also calls RVA 0x21EE20 and writes the party handler words +0x3C/+0x3E (RVA 0x15FCCC..0x15FD2D), consumed by party placement RVA 0x204462..0x204559.

Requests: helper RVA 0x15F760(slot, window) zeroes the remembered edge then docks. Callers: RVA 0x12AEA8 and 0x12B687 (`playermo`, vtable 0x333318 slots 5 and 10), RVA 0x141062 (layout swap over `actionm1`..`actionm8` records at RVA 0x3700A8), RVA 0x150897 (`targetwi` open, vtable 0x334F18 slot 1), RVA 0x13C1D3 / 0x13C1F3 (`iteminfo` re-dock by flag), RVA 0x13E512 (adds 0x1000 when mode byte +0x27 == 0), RVA 0x13E4A4 (adds 0x10000000 when mode == 4).

Item card: handler global VA 0x10578204. Mode preset table RVA 0x13E590..0x13E67C (jump table RVA 0x13E68C): mode 0 and default (16, 240), mode 4 (16, 16); code-move helper RVA 0x1602F0; preset y saved to handler +0x372 at RVA 0x13E679 and written as the window bound (+0xBC = (0, y), +0xC0 = 0) at RVA 0x13C00E..0x13C042. Mode 0 opener RVA 0x13E520 is called from event instruction handlers RVA 0xB9D73, 0xB9E24, 0xB9F32 (operand 1 item id; zero closes the card at RVA 0xB9D51). Mode 4 opener RVA 0x13E4B0 via RVA 0x13E6D0, installed as the `query` item callback at RVA 0xB23F6. `query` is created by RVA 0x15EA30 from event instruction handlers RVA 0xB2323, 0xB238B, 0xB23E8, 0xB2458, 0xB2507. Suspension flag setter RVA 0x136870 called by event instruction handler RVA 0xB79C0 (set) and RVA 0xB7A20 / 0x14B8A (clear).

Flags: registry RVA 0x3722D8, 0x2C-byte entries, flags +0x24, copied to window +0x34 on every OpenMenu by RVA 0x15E950 (also +0x99 = flag 0x100000 at RVA 0x15E9FD). 0x1: focus pick RVA 0x15E7FD (override byte +0x98 from RVA 0x15DB80's second argument). 0x2: window input RVA 0x11843E..0x11845B (cancel sound RVA 0x37BF0 when byte +0x84; without the flag, close instance RVA 0x15E790(window, 1)). 0x8: RVA 0x15E667..0x15E692 sets byte +0x68 and the frame open counter (RVA 0x1193B0 zero / RVA 0x1193A0 duration); duration global VA 0x1036C190 set to 25 or 0 by RVA 0x19953B; close animation armed only when +0x68 at RVA 0x118802; open animation drawn at RVA 0x117AE6 / 0x117C12; cursor part drawn on byte +0x78 (RVA 0x117875), untouched by 0x8. 0x10000: cursor init RVA 0x118870 from both OpenMenu paths (RVA 0x15E3E8, 0x15E4C0) reading record memory RVA 0x11B330; memory written at close by RVA 0x118830 (called RVA 0x15E6F5), skipped for item state 0x10 or base state 0xF; SetCursor RVA 0x118E90 clamps to 1..count and sets highlight state 3 unless state 5. 0x20000 / 0x40000: RVA 0x15E694..0x15E6D2; open check RVA 0x15E0F0 (state 0x60 case RVA 0x15E156, 0x40000 scan RVA 0x15E1B9..0x15E1F3). 0x100000: scale RVA 0x1601D0 (W times 1/640, H times 1/480); scaled rect quirk at RVA 0x118FE7 (bottom computed from left). 0x200000: OpenMenu new-instance path RVA 0x15E4B0 (before cursor init RVA 0x15E4C0 and the handler open call RVA 0x15E4CE) calls RVA 0x1190F0, which calls the state setter RVA 0x118B60(state 0x10, id == item count +0x58). State 0x10: whole-input refusal RVA 0x11829D, direction refusal RVA 0x118328 (falls through to the logic call at RVA 0x1184A0), hit test skip RVA 0x118D54, normal draw colour RVA 0x117933.

Confirm routine RVA 0x210E30: opens via RVA 0x15E350 the name at its object +0x20, then RVA 0x118E90(2) unless its fourth argument is nonzero (RVA 0x210EC0..0x210ECD). Objects at its call sites: VA 0x10664520 (`comyn`, 55 sites incl. RVA 0x12BB06 Dismount, 0x1609B3 / 0x160A40 / 0x160927 power prompts, 0x1D6BA2 Remodel, 0x19B339 key reset), VA 0x10578438 (`merityn`), VA 0x10664524 (`resyn`, RVA 0x96F40 passes 1), VA 0x10664510 (`shopbuy`, RVA 0x1DE61E buy, fourth argument 0), VA 0x10664514 (`shopsell`, RVA 0x1DEBBE, fourth argument 0); call list in scratch `reader_kit/dock/confirm_callers.txt`.

Handler mapping from a registry-global write scan (scratch script `reader_kit/dock/vmap.py`); `playermo` identity from its select handler RVA 0x12BD10 issuing `/attack %d` and the Command menu dispatch RVA 0x12BAE4 (mounted-command-menu record); `comyn` as the object of confirm helper RVA 0x210E30 (handler global VA 0x10664520, opens the window named by the object's own name field, then puts the cursor on item 2 unless its default-yes argument is set), used by RVA 0x160960 / 0x1609F0 (Log Out / Shut Down). A ranged-attack helper RVA 0x13F9E0 closes `abiselec` and `abimenu` together before issuing `/shoot`; that does not by itself identify `abiselec`. `myroom` select RVA 0x145460 carries `/garden`, `/layout` and opens `jobchang` (RVA 0x14552E); its Remodel arm RVA 0x1454E7 uses the `roomlist` handler global VA 0x106631E0, whose show method RVA 0x1D6820 name-opens `roomlist`; its Delivery Box method RVA 0x145BA0 name-opens `mogpost` beside `/mailbox` and `/sendpost`. `jobcsel` was not reached from these handlers. `shopmain` select RVA 0x210BE0: item 1 / item 2 install the buy / sell content (VA 0x10664518 / 0x1066451C) into the `shop` object VA 0x10664508 via RVA 0x2107D0; cancel closes `shopmain`. List class shared by `magic`, `ability`, `emote`, `mount` (common close RVA 0x221F20, per-frame RVA 0x221F50, refresh RVA 0x222110); `magic` open RVA 0x22C370 opens `mgcsortw` / `blusortw` and `subwindo`.
