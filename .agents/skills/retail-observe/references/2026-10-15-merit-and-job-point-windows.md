# Merit Points and Job Points windows, retail-2026-09

Observed 2026-10-15. Scope: clean retail-2026-09 FFXiMain.dll read statically. No DAT files were present and no client or server was run, so strings are named by table and entry, never quoted, and DAT record contents are unverified. Covers the chain behind Status selector `statcom2` item 7 (Merit Points) and item 8 (Job Points): `merit1`, `merit2`, `merit3`, `meritcat`, `meritinf`, `levmerit`, `merityn`, `merit2ca`, `jbpcat`, `levmeri2`, plus the shared `sortyn`, `bluehelp`, `titlewin` and `helpwind` the chain drives. Wire offsets are body offsets (packet offset minus the 4-byte header); field names are LSB's.

## Windows, focus and Esc

Every window here is the DAT window record of the same name (kuluu `ffxi_dat::menu_windows::windows_from_root`) and sits at its DAT frame unless a row says otherwise. Each open in the chain activates the new window without closing its opener; keys go to the topmost activated window that is not display-only. "Esc closes" means Esc closes the window without consulting its input handler; the window's close effects still run.

| Window | Role | Opened by | Close effects | Registry flags and keys |
|---|---|---|---|---|
| `merit1` | selector, items 1 and 2 | `statcom2` item 7 | closes `levmerit` | 0x20000: Esc closes; cursor kept between openings |
| `levmerit` | display-only panel | `statcom2` item 7, right after `merit1` | none | 0x1 |
| `merit2` | selector: item 1 EXP, item 2 Limit Points | `merit1` item 2 | none | 0x20000: Esc closes; cursor kept |
| `meritcat` | scrolling list, 11 rows = DAT items 1..11 | `merit1` item 1 | closes `meritinf` and `bluehelp`; releases the merit string table | 0x30002: Esc to handler |
| `meritinf` | display-only panel | `meritcat` open | none | 0x1 |
| `merit3` | selector: item 1 Raise, item 2 Lower | `meritcat` select | none | 0x10000: Esc closes; cursor on item 1 each open |
| `sortyn` | shared Yes/No (also the item-sort confirm) | `merit3` select | none | 0x200002: Esc to handler |
| `merityn` | shared query Yes/No | `sortyn` Yes; `jbpcat` select | none | 0x31002: Esc to handler; y from dock slot 0x1000, x from DAT |
| `merit2ca` | scrolling list of jobs, 11 rows | `statcom2` item 8 | closes `levmeri2` and `bluehelp`; releases job point strings | 0x30002: Esc to handler |
| `levmeri2` | display-only panel | `merit2ca` open | none | 0x1 |
| `jbpcat` | two-page list for one job, 10 rows = items 1..10; items 11 and 12 are page tabs | `merit2ca` select | releases gift data | 0x30002: Esc to handler |

Esc through handlers: `meritcat` inside a category returns to the category list with the cursor back on that category; at the top level it closes `meritcat`. `merit2ca` and `jbpcat` close themselves. `sortyn` answers No. `merityn`, as opened by this chain, only plays the buzzer (no cancel answer).

`levmerit` placement: when the Monstrosity predicate holds as `statcom2` item 7 is selected, `levmerit` opens 20 px below its DAT frame y, x unchanged; otherwise at its DAT frame.

UI sounds are named by their sound id: 0001 (page toggle), 0002 (confirm), 0004 (buzzer).

## Predicates

- Mog House: the latest s2c 0x00A LoginState is 1 (MYROOM), 4 (JOBEXIT) or 5 (POLEXIT_MYROOM). `meritcat` and `jbpcat` sample it when they open.
- Monstrosity: s2c 0x00A IsMonstrosity is non-zero, or s2c 0x061 mjob_no is 23.
- Main job and level: s2c 0x061 mjob_no, mjob_lv. Level of job j: s2c 0x01B job_lev[j] for j 0..15, job_lev2[j] for j 16..23.
- Key item k held: bit k of the held flags from s2c 0x055 (GetItemFlag of table TableIndex, 512 ids per table).
- Status effect active: the id is in the player's own list from s2c 0x037 (BufStatus with BufStatusBits as high bits).

## Entry from statcom2

Evaluated once, when `statcom2` opens. Item 7 is enabled only after an s2c 0x063 Merits record has arrived with canUseMeritMode set. Item 8 is enabled only after an s2c 0x063 JobPoints record has arrived with bit 0 of access set and main job level is at least 99. Item 7 opens `merit1`, then `levmerit`; item 8 opens `merit2ca`.

## merit1, merit2, levmerit

`merit1` open: item 2 is disabled while the Monstrosity predicate holds. Item 1 opens `meritcat`. Item 2: if status effect 143 (level_restriction), 257 (besieged) or 269 (level_sync) is active, print client message 187 (no buzzer) and open nothing; otherwise open `merit2`. Client messages are GetText table 7 entries printed as a local system chat line with chat mode 0x7B (kuluu's table-9 client messages use 0x79).

`merit2`: while a Merits record exists, the item of the current mode is disabled and the other enabled. meritModeEnabled set disables item 2 and enables item 1; clear disables item 1 and enables item 2. The state is reapplied at open and whenever that bit changes. Item 1 sends c2s 0x0BE {Kind 2 ChangeMode, Param1 0 (EXP), Param2 0, Param3 0}; item 2 sends {2, 1 (Limit Points), 0, 0}. No confirmation and no handler sound; `merit2` stays open and the server's Merits reply flips the disabled item.

`levmerit` draws two right-aligned lines; offsets are screen pixels from its on-screen rect, equal to 512x448 design units at UI scale 1:
- right − 8, top + 28: "<limitPoints>/10000" in 0x80808080, or "----/10000" with no Merits record.
- right − 8, top + 68: "<meritPoints>/<maxMeritPoints> P", 0x802060a0 when meritPoints ≥ maxMeritPoints, else 0x80808080. Retail has no no-record form for this line; it is unreachable because item 7 requires the record.

Its labels are text from its own DAT record.

## meritcat rows

Identifiers. A merit's index is its wire id (s2c 0x08C merits[].index) halved; c2s 0x0BE Param2 carries this index. The category of a stored merit is its wire id shifted right by 6, which equals the merit index shifted right by 5. The client's per-merit state is the latest s2c 0x08C entry: count (current upgrades) and next (cost of the next upgrade).

DAT sources:
- Category records: DAT file 89, read whole as 96 records of 12 bytes. Fields used: u16 at byte 0 (category id, also the name key), signed byte 3 (category id used for lookup), byte 4 (upgrade cap; 0 means absent).
- Merit definitions: DAT file 88, a 12-byte record at offset 12 × merit index. The u16 at byte 0 must equal the index, byte 4 is the max upgrade count (0 means absent), and the u32 at byte 8 is a job bitmask (bit n = job n).
- Names and descriptions: d_msg catalog 55686 (Japanese 55566); entry 2k is the name and 2k+1 the description. For merits, k is the merit index. For categories, k is the category record's byte-0 id. `meritcat` loads it on open and releases it on close.

Top level: one row per category record number 1..31, in record order, skipping absent records and records whose byte-0 id is 0. The row shows only the name, in 0x80808080; the row id is the byte-0 id. Selecting a row enters that category with the cursor on its first row and plays 0002.

Inside category c, with section ids s:
- c from 1 to 5, or c = 26: one section, s = c, with no header row.
- Every other c (job categories): sections s = c, c + 26 and c + 58, in that order. Each section with a category record (matched on byte 3) starts with a header row, then lists its merits.
- A section lists merit indices 32s through 32s + 31 that have a definition record.
- A header row shows a graphic from UI element group `menu    windowps` at column 0 (element 592 for the first section, 593 for the second and third) and the section's cap (byte 4) right-aligned at x 340. Selecting a header row does nothing.

Row geometry for `meritcat` and `merit2ca`: x from the window's left edge, visible row k (0-based) at y = top + 4 + 16k, names drawn at row y + 2, numbers at row y + 4. The cursor on item k shows row scroll + k − 1.

| Cell | Content |
|---|---|
| name | string 2 × index at x 6, left-aligned |
| count | current upgrades right-aligned at x 238; "0" when no stored entry |
| cost | blank when next is 0; else caption GetText table 12 entry 90 at x 260 and "<next> P" right-aligned at x 318 |

Colors. E = Mog House at `meritcat` open, AND the definition's job bitmask has the main job's bit, AND, for section 26 only, main job level ≥ 96 with key item 2059 (HEART_OF_THE_BUSHIN) held.
- name: section color if E (first 0x80808080, second 0x80909040, third 0x80509050), else 0x80404040.
- count: 0x802060a0 when count equals the definition's max, else the name color.
- cost: 0x80808080 when E holds, a Merits record exists, next ≤ meritPoints, and section total < section cap; else 0x80404040. Section total is the sum of count over stored merits of section s. Section cap is byte 4 of the record for s; for s = 26 it is +5 for each of key items 3224 (PRIMER_ON_MARTIAL_TECHNIQUES) and 3225 (TREATISE_ON_MARTIAL_TECHNIQUES), capped at 100.
- The cursor row's name is drawn in 0x80803040 instead (also in `merit2ca` and `jbpcat`).

Selecting a merit row:
- Outside the Mog House at open: client message 178, then the buzzer.
- Else, when Raise or Lower is possible, open `merit3` and play 0002; otherwise the buzzer.
- Raise is possible when next ≠ 0 and the cost cell is 0x80808080. Lower is possible when count ≠ 0 and E holds.

Rebuild. Rows rebuild on open, on entering or leaving a category, and whenever an s2c 0x08C changes the stored merits; a rebuild keeps the cursor row position. A Merits record alone does not rebuild rows. `meritcat` reopens on the category row that was highlighted when it last closed.

`meritinf` lines, offsets from its rect:
- right − 8, top + 14: "<meritPoints> P", or "- P" with no Merits record; 0x80808080.
- right − 8, top + 40: "<total>/<cap>" for category c. The total is drawn in 0x802060a0 when it equals the cap, else 0x80808080; "/<cap>" is 0x80808080.
- Job categories only: right − 64, top + 40, the same form for section c + 26 in 0x80909040.

At the top level c is the highlighted row's id; inside a category the panel keeps the category that was entered. The cap on both lines comes from category record number (highlighted top-level row position + 1), with the section 26 key item bonus when that record's byte 3 is 26.

`bluehelp` shows the highlighted row's description (entry 2k + 1) with its name (entry 2k) as title. At the top level k comes from category record number (row position + 1). Both this and `meritinf` assume records 1..N are contiguous; unverified without DATs.

## merit3, sortyn and merityn: raising and lowering

`merit3` open: item 1 (Raise) is disabled unless Raise is possible and item 2 (Lower) unless Lower is possible; if neither is possible `merit3` closes itself. Every refresh re-evaluates both and re-enables items. It closes `merityn`, `sortyn` and itself together when neither action remains possible, or when the last item chosen (Raise or Lower) has become impossible.

Raise, item 1:
1. `sortyn` opens. `titlewin` shows GetText table 13 entry 130 and `helpwind` shows GetText table 14 entry 612; cursor on item 2. Item 1 is Yes; item 2 or Esc is No, which closes `sortyn` and returns to `merit3`.
2. On Yes, if the merit has a stored s2c 0x08C entry: `merityn` opens on item 2 with title GetText table 13 entry 130. Its text is GetText table 10 entry 80, with the merit name as the string parameter and next as the number. Esc plays the buzzer.
3. `merityn` item 1 sends c2s 0x0BE {Kind 3 EditMode, Param1 1 Raise, Param2 merit index, Param3 0} and closes `merit3`. Item 2 returns to `merit3`.

Lower, item 2: the same flow with help entry 613 and text GetText table 10 entry 81 (merit name only), sending {3, 0 Lower, index, 0}.

## merit2ca, levmeri2, jbpcat: job points

Sources:
- Job categories: DAT file 94, 32 records of 68 bytes. Byte 2 is the job id (0 means empty) and bytes 4..67 hold 32 u16 job point type ids (0 means an empty slot). `jbpcat` reads record number j directly for job j.
- Type definitions: DAT file 93, a 4-byte record at offset 4 × type id. The u16 at byte 0 must equal the type id; byte 3 is the max level.
- Names and descriptions: d_msg catalog 55694 (Japanese 55574); entry 2k is the name and 2k+1 the description. k is the job id for jobs and the type id for types.
- Type state: s2c 0x08D points[] gives level and next per type id = job_no × 32 + index.
- Job totals: s2c 0x063 JobPoints jobs[j] {capacityPoints, currentJp, totalJpSpent}.
- Gifts of job j: thresholds in DAT file 100407 + j (256 u16, required JP spent, 0 = no gift). Gift i's name is d_msg catalog 55674 (Japanese 55554) entry 2 × (256 × (j − 1) + i), and its description is the next entry.

`merit2ca` on open:
- Sends c2s 0x0C0 (header only) only if no s2c 0x08D has been processed since the client's merit and job point state was last reset.
- Opens `bluehelp` and `levmeri2`.

Its rows, one per job category record 0..31 in order with a non-zero job id: the job name (string 2 × job) at x 6, in 0x80808080 when that job's level is ≥ 99, else 0x80404040; the row id is the job. Selecting job 1..22 at level ≥ 99 plays 0002 and opens `jbpcat` for that job on its Job Points page with the cursor on the first row. Any other row plays the buzzer. `bluehelp` follows the highlighted job: description 2 × job + 1, title 2 × job.

`levmeri2`, for the highlighted job j (1..31) when a JobPoints record exists:
- right − 8, top + 28: "<capacityPoints>/30000" in 0x80808080.
- right − 8, top + 68: "<currentJp>/500 P", 0x802060a0 when currentJp ≥ 500, else 0x80808080.

`jbpcat` layout: a page graphic from group `menu    windowps` sits at its top-left, element 722 on the Job Points page and 723 on the Gifts page. Rows are at y = top + 17 + 17k, with the same name and number offsets as `meritcat`. The Gifts page also draws totalJpSpent of job j, left-aligned at left + 258, top + 4.

Job Points page: one row per non-zero type id t in job j's record, in slot order, skipping types without a definition.

| Cell | Content |
|---|---|
| name | string 2t at x 6 |
| level | right-aligned at x 252 |
| max | "/<max>" left-aligned at x 253 |
| cost | caption GetText table 12 entry 90 at x 290 and "<next> P" right-aligned at x 357 |

S = Mog House at `jbpcat` open and not Monstrosity. HasData = stored level ≠ 0 or next ≠ 0.
- Not S, or no HasData: name, level and max in 0x80404040, cost blank.
- Otherwise name, level and max in 0x80808080. At level ≥ max, level and max turn 0x802060a0 and the cost is blank. Below max, the cost is 0x80808080 when a JobPoints record exists and currentJp(j) ≥ next, else 0x80404040.

Gifts page: one row per gift i (0..255) with a non-zero threshold. The gift name is at x 6 and the threshold right-aligned at x 342; both are 0x80808080 when totalJpSpent(j) ≥ threshold, else 0x80404040.

Page switching: Left, Right, or selecting item 11 or 12 toggles the page and plays 0001. Each page keeps its cursor row while `jbpcat` stays open.

Selecting a Job Points row:
- Not S: client message 178, then the buzzer.
- S, level < max, a JobPoints record, HasData and currentJp ≥ next: play 0002 and open `merityn` on item 2 with no title. Its text is GetText table 10 entry 116, with the type name and next. Esc plays the buzzer. Yes sends c2s 0x0BF {Index = type id, padding 0} and starts the spend lockout; No returns.
- Any other row, including Gifts page rows: the buzzer.

Spend lockout: after a confirmed spend, selecting a raisable row still plays 0002 but opens no prompt. This lasts until a counter of 30 runs out; it decreases by one per refresh pass of the focused `merit2ca` or of `jbpcat` while focused on its Job Points page.

## Wire

| Packet | Body | Client use |
|---|---|---|
| c2s 0x0BE, 12 bytes | Kind u8, Param1 u8, Param2 u16, Param3 u32 | ChangeMode {2, 0 or 1, 0, 0}; EditMode {3, 1 raise or 0 lower, merit index, 0}; no other kind is sent |
| c2s 0x0BF, 8 bytes | Index u16, padding u16 | Index = type id |
| c2s 0x0C0, 4 bytes | none | sent by `merit2ca` open, see above |
| s2c 0x08C | merit_count, merits[] {index, next, count} | merits are stored keyed by index with bit 0 ignored. An entry with bit 0 set deletes its merit; any other entry inserts or replaces. Any change makes `meritcat` rebuild |
| s2c 0x08D | 64 × {index:5, job_no:11, next:10, level:6} | skips entries whose first u16 is 0 or whose job_no ≥ 24; stores level and next per type; marks job point data received; an open `jbpcat` on the Job Points page of an updated job rebuilds |
| s2c 0x063 type 2 Merits | limitPoints, merit bit-word, maxMeritPoints | replaces the stored record |
| s2c 0x063 type 5 JobPoints | access, jobs[24] | replaces the stored record; an open `jbpcat` on its Job Points page rebuilds |

The `jbpcat` rebuild hooks exist only while `merit2ca` is open, which is always the case while `jbpcat` is.

## Refusals

| Where | Predicate | Effect |
|---|---|---|
| `statcom2` items 7 and 8 | entry predicates fail at `statcom2` open | item disabled |
| `merit1` item 2 | Monstrosity at `merit1` open | item disabled |
| `merit1` item 2 | effect 143, 257 or 269 active | message 187, no buzzer |
| `merit2` | the item of the current mode | item disabled |
| `meritcat` merit row | outside the Mog House at open | message 178, buzzer |
| `meritcat` merit row | neither Raise nor Lower possible | buzzer |
| `merit3` items | Raise or Lower impossible | item disabled; window closes when neither is possible |
| `merityn` | Esc | buzzer |
| `merit2ca` row | job outside 1..22 or level < 99 | buzzer |
| `jbpcat` row | not S | message 178, buzzer |
| `jbpcat` row | maxed, unaffordable, no data, or Gifts page | buzzer |
| `jbpcat` row | spend lockout running | 0002 plays, no prompt |

## String and graphic sources

| Value | Source |
|---|---|
| merit and category names, descriptions | d_msg 55686 (Japanese 55566), entries 2k, 2k+1 |
| job and job point names, descriptions | d_msg 55694 (Japanese 55574) |
| gift names, descriptions | d_msg 55674 (Japanese 55554) |
| cost caption | GetText table 12 entry 90 (`GETTEXT_TABLE_12_CATALOG_FILE_ID`, 55653) |
| merit prompt titles | GetText table 13 entry 130 (`CAPTION_SHORT_CATALOG_FILE_ID`, 55652) |
| `sortyn` help line | GetText table 14 entries 612 (raise) and 613 (lower) (`CAPTION_CATALOG_TABLE_14_FILE_ID`, 55651) |
| `merityn` texts | GetText table 10 entries 80, 81, 116: English file 50, Japanese file 32, read as a raw string table with per-entry parameter counts (not d_msg) |
| client messages 178, 187 | GetText table 7, d_msg 55646 (Japanese 55526) |
| section and page graphics | UI element group `menu    windowps` elements 592, 593, 722, 723 |

The English d_msg ids above are on the same language row as kuluu's table-9 catalog 55648.

## Established vs not established

Established: the window chain, open and close cascades, and Esc routing above. All row sources, cell contents and colors, every gate and refusal, the exact c2s bodies and their trigger points, and how s2c 0x08C, 0x08D and both 0x063 types update the stored state and the open windows.

Not established:
- String contents and DAT record contents. No DATs were available, including whether category records 1..N are contiguous, which the description and cap lookups assume.
- The second English language row. It applies when a client language configuration bit is clear and maps table 7 to 55766, merit strings to 55806 and job point strings to 55814.
- Which DAT holds the `windowps` element group.
- The binary layout of the GetText table 10 file and its parameter syntax.
- Whether the client's merit and job point state, and so the "0x08D received" flag, resets on every zone change or only at session teardown.
- `bluehelp` placement, since it opens with no explicit position.
- The input-binding test that makes Left/Right page-scroll instead of toggling pages in `jbpcat`.
- A `jbpcat` window flag that turns row select into a buzzer. It starts on and is cleared only by the pointer path targeting items 11 and 12.
- The meaning of registry flag 0x200000 on `sortyn`.
- Exact on-screen rendering of chat mode 0x7B.
- Side note for chat work: merit names inside s2c 0x029 messages resolve through the merit string table, which retail holds only while `meritcat` is open.

## Provenance

All code locations are RVAs in retail-2026-09 (image base VA 0x10000000, RVA = VA − 0x10000000). Packed DLL SHA-256 f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4; unpacked `.text` SHA-256 b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9; TDS 0x6A995428. Read with capstone from `/home/claude/ffximain/FFXiMain.unpacked.dll`; no process was run, no source edited, nothing committed. Scratch scripts live under the session scratchpad `reader_kit/merit/`.

Registry (0x2C-byte entries, name/global/flags/mode/priority/group): statcom2 RVA 0x374278 (VA 0x10578404, 0x220002, 1/2/0x81); meritcat 0x3742A4 (0x10578408, 0x30002, 1/2/0x1C); merit1 0x3742D0 (0x1057840C, 0x20000, 1/2/0x81); merit2 0x3742FC (0x10578410, 0x20000); merit3 0x374328 (0x10578414, 0x10000); levmerit 0x374354 (0x10578418, 0x1, 1/2/4); meritinf 0x374380 (0x1057841C, 0x1); merityn 0x3743AC (0x10578438, 0x31002, 0x41/8/0); merit2ca 0x3743D8 (0x10578420, 0x30002, 1/2/0x1C); levmeri2 0x374404 (0x10578424, 0x1); jbpcat 0x374430 (0x1057842C, 0x30002); sortyn 0x374EAC (0x10666EA4, 0x200002); bluehelp 0x373D24 (0x106636A0, 0x3); helpwind 0x3727D4 (0x105781F8); titlewin 0x372800 (0x1057847C). Handlers built at RVA 0x146590..0x146880: vtables merit1 0x334730 (open 0x149830, close 0x149850, select 0x149860), merit2 0x3346E8 (open 0x146990, select 0x149950, refresh 0x1498F0), merit3 0x3346A0 (open 0x149980, select 0x149AD0, refresh 0x1499F0), levmerit 0x334658 (shared level class 0x334970: open 0x147620, close 0x1479C0; draw 0x149F20; ctor 0x1475B0), meritinf 0x334610 (open 0x149E50, draw 0x149E60), levmeri2 0x3345C8 (open 0x14A030, draw 0x14A040), meritcat 0x334A48 (ctor 0x148ED0), merit2ca 0x334AB8 (ctor 0x14A170), jbpcat 0x334B28 (ctor 0x14A7D0), merityn 0x33BC30 (ctor 0x210DA0 with name RVA 0x371888, byte arg 0), sortyn 0x33C000 (open 0x2227D0, close 0x222840, input 0x2228B0, refresh 0x222880). Close instance RVA 0x15E790 calls the handler close slot through 0x118F30; refresh dispatch 0x15FDF0..0x15FE05 passes 1 only to the focused window (VA 0x1062188C).

statcom2: select 0x148D00 (item 7 path 0x148E18..0x148E5D opens table RVA 0x3716B8 entry 6 "merit1", sets levmerit mode byte 1 at 0x148E40 when 0x97D50 is true, opens "levmerit" RVA 0x37193C; item 8 opens entry 7 "merit2ca"); open 0x148E80 disables item 7 unless 0x991D0 returns a record with byte 3 bit 0x20 (bit 13 of the Merits bit-word), item 8 unless 0x991F0 returns a record with byte 0 & 1 set and VA 0x10485641 ≥ 0x63; refresh 0x148BE0 never touches items 7/8. Predicates: Monstrosity 0x97D50 (game VA 0x104DFD98 field +0x40DB6, written at 0xFB45C..0xFB460 from s2c 0x00A packet +0x7E, or VA 0x10485640 == 0x17); main job/level VA 0x10485640/0x10485641 written by s2c 0x061 handler 0x994A0 from packet +0xC/+0xD; Mog House 0x1CC070(0) tests bit 0x100 of game +0x3C35C (0xFC160), set by the 0x00A switch on packet +0x80 at 0xFAD8F..0xFAE15 (case table 0xFB640: 1 → 0x1|0x100, 3 → 0x200, 4 → 0x2000|0x100, 5 → 0x200|0x100, 2 → none); status effect test 0x21E670 over 32 u16 at VA 0x10485A30 filled by s2c 0x037 handler 0x9D0F0; key item test 0x97E80 → 0x97E50 over VA 0x1047FEB8 filled by s2c 0x055 handler 0x98710 (+0x84 table index); job level 0xF0FB0 (game +0x1A628[j], +0x1A660[j]) filled by s2c 0x01B handler 0xF0940 from packet +0x10/+0x48.

merit1/merit2/levmerit: 0x149830 sets item 2 state 5 under 0x97D50; 0x149850 CloseMenu levmerit; 0x149860 item 1 → OpenMenu RVA 0x3719AC "meritcat", item 2 → 0x21E670(0x8F/0x101/0x10D) → 0x129E30(0xBB) on VA 0x10578158 else OpenMenu RVA 0x3719C0 "merit2". Emitter 0x129E30: GetText(7, id < 0xC9), bytes (0x7B, 3, 0) to 0x127C00; table-9 emitter 0x129DE0 uses (0x79, 3, 0). merit2 refresh 0x1498F0 reads Merits word at record +2 bit 15, item states 5/0xB, cache +0x14 reset to −1 by open 0x146990; select 0x149950 → 0xE4B60(0/1). levmerit open 0x147620: DAT position via 0x11A710, mode 1/2 add RVA 0x3716A4 (20) / 0x3716A8 (52) to y, setter 0x119110, rect via 0x118F70 into +0x14; close 0x1479C0 clears mode. Draw 0x149F20: "%d/%d" RVA 0x357FBC with 0x2710, "----/%d" 0x3719F4, "%d/%d P" 0x3719EC, no-record branch formats "-/%d P" 0x3719E4 from absolute address 4; 0x802060A0 when byte +2&0x7F ≥ byte +4.

meritcat: ctor 0x148ED0 (0x1F6700(0xB) visible rows, column table RVA 0x371694 = 0,6,16,220,240,260,320,342 for 7 columns, row height +0x26 = 0x10, margins +0x2C/+0x2D = 0/4, +0x47 = +0x48 = 1). List-class hooks (vtable indices 17/19/20/22): open 0x149580 (0x149040(0), +0x52 = 0x1CC070(0), section graphics from 0x1C8C30(1) element table +0x940/+0x944/+0x944 into +0x60/+0x64/+0x68, strings 0x160E40, bluehelp 0x1EC220(−999,−999,0,0), OpenMenu RVA 0x371984 "meritinf"), close 0x149610, refresh 0x149690 (version via 0xE4C60 → rebuild 0x149490; bluehelp text 0x1610C0 and title callback 0x149650; meritinf +0x14/+0x18 only at top level), input 0x149750 (top level 0x149040(id) + 0x37B90; 0x1490C0/0x149130 → OpenMenu RVA 0x371998 "merit3" + 0x37B90, else 0x37C50; +0x52 clear → 0x129E30(0xB2) + 0x37C50; Esc → 0x15E790 or 0x149040(0)). Section builder 0x1491A0: job sections at 0x1494FD..0x149538 (cat, cat+0x1A, cat+0x3A with header flag), header cells kinds 0xE/7 at 0x149264..0x1492A7, WS cap bonus 0x14921B..0x149251 (0xC98/0xC99, clamp 0x64), WS gate 0x14936E..0x149391 (level 0x60, key item 0x80B), job mask test 0x149397, section colors RVA 0x371858, cost checks 0x1493A2..0x1493D5, max color 0x1493F9, cells 1/3/5 kinds 0x1E/8/0x1F at 0x149419..0x14944D. Generic list class (vtable RVA 0x33A730): open 0x1F68B0, close 0x1F6980, draw 0x1F6A60 (cursor-row column +0x48 drawn 0x80803040 through 0x1F4BB0), input 0x1F6C70 (hook +0x58, then for code 5 hook +0x5C decides 0x37B90/0x37C50; Right/Left paging via 0x123A70 unless +0x47), refresh 0x1F69D0, row records 0x54 bytes filled by 0x1F4B20. Cell kinds via table RVA 0x1F6574: 7 0x1F4CEA ("%d" RVA 0x352FF0), 8 0x1F4D46 (data−1), 0xE 0x1F4F78, 0x1E 0x1F5740 (0x161070), 0x1F 0x1F578E (GetText(0xC,0x5A) then "%d P" RVA 0x3719DC), 0x35 0x1F5E87 (0x161250), 0x36 0x1F6471, 0x37 0x1F64C3 ("/%d" RVA 0x3719D4), 0x38 0x1F6509, 0x39 0x1F5EA5 (0x1614C0), 0x3A 0x1F5EF4 (+0x34). meritinf: open 0x149E50 (rect +0x1C), draw 0x149E60 ("%d P"/"- P" RVA 0x3719DC/0x3719D8, group 2 at 0x149EFA with 0x80909040), totals 0x149D40. bluehelp: 0x1EC220, 0x1EC2D0, text 0x1EC330, clear 0x1EC3C0.

Merit table object VA 0x106218FC (ctor 0x160DB0, vtable RVA 0x335334, created at 0x1650C9): category records from file 0x59 at 0x160CA0 (0x480 bytes into +0x19C); definition records file 0x58 (slot functions 0x160CF0 file id, 0x160DD0 offset id×12, 0x160DE0 size 12, 0x160D40 match u16, 0x160D60 validate byte +4); strings 0x160E40 (d_msg class 0x1C8260/0x1C8380, language index 0x1D) released at 0x160EA0; name 0x161070 (arg < 0x20 → record by index 0x160F00, else 0x160EF0; string 2×u16), description 0x1610C0; record by category 0x160F30 (byte +3); state 0x160FB0 (stored index >> 1); total 0x161010 (stored index >> 6, sum of byte +3).

merit3/sortyn/merityn: merit3 open 0x149980, refresh 0x1499F0 (closes merityn, sortyn VA 0x10666EA4 window, merit3), select 0x149AD0 (GetText(0xE, 0x264 + (item ≠ 1)), GetText(0xD, 0x82), sortyn setup 0x222770 with callbacks 0x149B90/0x149C70). Raise 0x149B90: formatter 0x153180 (→ 0x153A40) table 0xA id 0x50 with name + next into VA 0x105782E0, merityn query 0x210E30 (title GetText(0xD,0x82), callback 0x149C40 → 0xE4BA0 + CloseMenu RVA 0x371998). Lower 0x149C70: id 0x51, callback 0x149D10 → 0xE4BC0. sortyn: setup moves the cursor to item 2 when arg 5 is 0; input 0x2228B0 answers item 1 yes, item 2 or Esc no, then CloseMenu RVA 0x38610C; open/refresh set titlewin (0x150CC0) and helpwind (0x13A320). merityn query 0x210E30: busy check +0x14, cursor item 2 when arg 4 is 0, cancel flag +0x39 = +0x38 (ctor byte 0) when arg 6 is non-zero; input 0x2110C0 code 5 → 0x211370 (answer = cursor item 1), code 6 → 0x37C50 when +0x39 is 0.

Packets: c2s 0x0BE builder 0xE4B00 (alloc 0xFC3C0(0xBE), payload +4 kind, +5 param1, +6 param2, +8 param3 & 0xFFFF, commit 0xFC4D0 size 0xC); ChangeMode 0xE4B60, raise 0xE4BA0, lower 0xE4BC0; 0xE4AE0 (kind 1) and 0xE4B80 have no references anywhere in the file. c2s 0x0BF 0xE4F60 (+4 index, size 8), sole caller 0x14B61D; c2s 0x0C0 0xE4FA0 (size 4), sole caller 0x14A2B8. s2c 0x08C handler 0xE4CA0 (registered 0xE4340 with 0x08D 0xE4FD0 and 0x08E 0xE5140): sorted array game +0x2D000, count +0x2CFFE, capacity +0x2CFFC; low bit set deletes; version +0x2D004 = 0x1039E0() on change; callback +0x2D008. s2c 0x08D handler 0xE4FD0: 64 entries from packet +4, job +0x18 limit, u16 per type at game +0x2D010 (level bits 0..5, next bits 6..15), per-job callback +0x2D610, flag game +0x40DB5 bit 0. s2c 0x063 handler 0x99270: type 2 stores packet +8/+0xC at VA 0x10485826/0x1048582A, flag VA 0x10485824 bit 1; type 5 copies 0x25 dwords from packet +8 to VA 0x10485830, bit 2, then calls game +0x2D610 for jobs 1..0x16; getters 0x991D0/0x991F0. State reset 0xE43F0/0xE4490 clears merits, the type array, bit 0 and the callback; called from teardown 0xF9E10 (ends with VA 0x104DFD98 = 0) and the init path 0xE4340; s2c 0x00A writes +0x40DB5 bits 1..3 only (0xFB43F..0xFB456).

Job points: merit2ca ctor 0x14A170 (visible 0xB, columns RVA 0x371864 = 0,6,252,253,290,357, row height 0x10, margins 0/4); hooks open 0x14A2A0 (0x14A280 tests +0x40DB5 bit 0 → 0xE4FA0; bluehelp; 0x1611A0; build 0x14A650(0) → rows 0x14A700 → OpenMenu RVA 0x3719FC "levmeri2"; game +0x2D610 = 0x14A330; lockout VA 0x1057844C = 0), close 0x14A360, refresh 0x14A3F0 (focused only; lockout decrement), input 0x14A580 (job gate 0x14A550: 1..0x16 and 0xF0FB0 ≥ 0x63; 0x37B90 then jbpcat 0x14AF70 setting job +0x54, page +0x58 = 0, OpenMenu RVA 0x371A10). Row build 0x14A700 also computes GetText(0x12, job) and discards it. JP table object VA 0x10578428 (ctor 0x161110, vtable RVA 0x335350): categories file 0x5E (0x880 bytes, 0x44-byte records, 0x1612A0), definitions file 0x5D (+0x20, 4-byte records via 0x161290, match 0x161350), strings language index 0x1E (0x1611A0), name 0x161250, description 0x161270, state pointer 0x1612D0. Gift object VA 0x10621900 (ctor 0x161380, vtable RVA 0x33536C): load 0x1613D0 (strings language index 0x1F; thresholds file 0x18837 + job, 0x200 bytes), release 0x161470, name 0x1614C0, description 0x161500, threshold 0x161540. levmeri2 draw 0x14A040 (jobs[j] +4+6j and +6+6j, "%d/%d" with 0x7530, "%d/%d P" with 0x1F4). jbpcat: ctor 0x14A7D0 (visible 0xA, row height and y margin 0x11); open 0x14A8C0 (+0x5E = Mog House and not 0x97D50; +0x47 = 0), close 0x14A920, refresh 0x14AAB0 → 0x14A9D0 (page 0, lockout decrement) / 0x14AA40 (page 1), input override 0x14AAE0 (0x123A70(0x3E,0xA8,4,−1) remaps codes 3/4 to 0x19/0x18), input hook 0x14ACC0 (table RVA 0x14AF34/0x14AF4C; items 0xB/0xC and codes 3/4 toggle with 0x37B30; window +0x62 gate at 0x14ACFF, initialised 1 at 0x11738E; select 0x14AD0A..0x14AD7D), draw 0x14ABD0 (0x1C8C30(1) elements +0xB48/+0xB4C, total spent at +0x102/+4), builders 0x14B0D0 (page 0) and 0x14B300 (page 1), maxed 0x14B440, can raise 0x14B490, has data 0x14A260, spend prompt 0x14B580 (lockout test; GetText(0xA, 0x74) with name and next; callback 0x14B610 → 0xE4F60, lockout = 0x1E).

Shared: UI sounds 0x37B30/0x37B90/0x37BF0/0x37C50 push sound ids "0001".."0004". GetText 0x152EF0 → 0x152F20, table registry RVA 0x371DA8 ((7, VA 0x10578688), (0xC, VA 0x10578698), (0xD, VA 0x10578518), (0xE, VA 0x10578508)); object loads at 0x153C80.. through language lookup 0x25B560 (table 7 index 7, 0xC index 0xE, 0xD index 0xD, 0xE index 0xC); table 10 is a raw buffer loaded by 0x152FF0 from file 0x32 (0x153B53, English) or 0x20 (0x153B13, Japanese) and read by 0x1530C0. Language rows from 0x25B4F0: RVA 0x38A548 (English, configuration bit set; index 7 = 55646, 9 = 55648, 0xC/0xD/0xE = 55651/55652/55653, 0x1D = 55686, 0x1E = 55694, 0x1F = 55674), RVA 0x38A388 (English, bit clear; 55766, 55768, 55771..55773, 55806, 55814, 55674), RVA 0x38AB08 (Japanese; 55526, 55566, 55574, 55554). 0x1C8C30(1) is VA 0x10662E54, group "menu    windowps" (RVA 0x36FFB4) bound by 0x120450, element pointers at group +0x20 (0x11E290). Merit-name message parameter resolver 0xBD9F0 → 0x161070 (caller 0xC2070). LSB headers used: `vendor/server/src/map/packets/c2s/0x0be_merits.h`, `0x0bf_job_points_spend.h`, `0x0c0_job_points_req.h`; `vendor/server/src/map/packets/s2c/0x08c_merit.h`, `0x08d_job_points.h`, `0x063_miscdata.h`, `0x063_miscdata_merits.h`, `0x063_miscdata_job_points.h`, `0x061_clistatus.h`, `0x00a_login.h`, `0x01b_job_info.h`, `0x055_scenarioitem.h`; `vendor/server/src/map/packets/char_status.cpp` (GP_SERV_SERVERSTATUS); ids from `vendor/server/data/merits.yaml`, `vendor/server/data/status_effects.yaml`, `vendor/server/scripts/enum/key_item.lua`, `vendor/server/src/map/job_points.h`.
