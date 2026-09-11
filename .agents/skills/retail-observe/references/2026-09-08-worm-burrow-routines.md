# Worm burrow routines and animation dispatch, 2026-09-08 and 2026-09-09

How the retail client turns `animationsub`, status, and action packets into
named scheduler routines, observed on Carrion Worms and Forest Hares.
Kuluu code cites sections of this record by heading.

## Method and naming

Runtime: HorizonXI, 2026-09-08 and 2026-09-09. An Ashita capture addon logged
every 0x0E and 0x28 packet and read a set of per-entity and per-actor memory
fields once per client frame (one frame is about 34.5 ms, the client ran near
29 fps). Six Carrion Worms across three runs and six Forest Hares across one;
the addon and its logs are not in this tree.

Static: 2026-09-08, gate probes and full-function dumps of a retail
`FFXiMain.dll` taken from a PhoenixXI install (build not recorded). Addresses
below are RVAs for that build and do not line up with the HorizonXI VAs in
[2026-09-08-npc-animation-selector.md](2026-09-08-npc-animation-selector.md),
which decodes the same dispatch table for the installed client.

Field names used throughout:

| Name | Location | Meaning |
| --- | --- | --- |
| ActionTimer1 | entity `+0x11C` (u16) | count of scheduler routines currently holding the actor |
| RF0 | entity `+0x120` | status and visibility bits |
| RF1 | entity `+0x124` | bits 1-3 hold the live `animationsub`; bits 4-6 mirror it; bits 13-14 hold a second 2-bit copy |
| RF2 | entity `+0x128` | dispatch latches (bits 29, 30, 31) |
| RF3 | entity `+0x12C` | bit 0 rebuild pending, bit 23 pop effect, bit 28 set by status changes |
| RF4 | entity `+0x130` | bit 7 = "sub != 0" latch |
| Status | entity `+0x170` | the raw state the dispatch jump tables key on |
| sub byte | actor `+0x8A9` | the actor's own copy of `animationsub` |

## Sub-to-routine dispatch

The table at RVA `0x35AF60` (`.data`) is an inline array of eight FourCCs,
`[init, ini1, ini2, ini3, init, ini1, ini2, ini3]`, indexed by RF1 bits 1-3
(`shr eax,1; and al,7` at RVAs `0x8C5C8` and `0x8F091`). Entries 4-7 repeat
0-3, so the index wraps mod 4. Verified byte for byte against the on-disk
`.data` section.

- **Normalizer** at RVA `0x8EF70(entity)`: reads the live sub from RF1 bits
  1-3, keeps a mirror in bits 4-6 (`RF1 := low_nibble | (sub << 3)`), syncs the
  actor's sub byte through the setter slot, and routes to the change detector
  unless in a steady state (sub 4 with nibble 0, or sub 0 with nibble 4).
- **Change detector** at RVA `0x8EFD5`: compares RF1 bits 1-3 against the
  actor's sub byte and the mirror nibble. On change it stores the new sub and
  plays `table[sub]` through the named-play vtable slots `+0x29C` and
  `+0x298`, gated by RF0 bit 9 (plus RF4 bit 2 when RF0 bit 13 is clear), a
  check call through slot `+0x2A0(actor, fourcc)`, and for sub >= 4 by RF2
  bit 29.
- **State dispatch** at RVA `0x8C490(entity)`. Prologue: while RF2 bit 31 is
  clear and the byte at `+0xEE` is not in {3, 4, 5} and the predicate at
  `0xD12E0(actor)` holds, call `0xCF110(entity, 0)` and set bit 31 (the
  initialized latch). Entry is skipped while RF2 bit 29 (dispatched this
  cycle) is set; bit 29 is set at `0x8C61A` after processing and cleared
  together with bit 30 by `and [esi+0x128],0x5FFFFFFF` at `0x90CAD` on the
  destroy path. A jump table keyed on `Status - 6` (byte map at RVA `0x8C680`,
  table at `0x8C670`) selects:
  - raw states 6, 50, 56: the fishing helper at `0x8C6D0(entity, state)`
    plays `fsh0`..`fsh3` through slot `+0x29C`;
  - raw state 34: plays `init` then the literal `inte` through slot
    `+0x298` and clears actor `+0x7D8` to spaces (meaning of `inte` unknown);
  - raw states 64-83: `0xD60D0` loops nine times over `0xD60F0`, which builds
    `wep0`..`wep8` (weapon-attack clips);
  - every other state: the sub block at `0x8C5A8` saves the old sub, stores
    the new one from RF1 bits 1-3, plays `init` through slot `+0x298`, runs
    the post-init hook `0xCF070(entity, actor)`, clears actor `+0x7D8`, then
    stores the old sub back. The restore is unexplained; the best reading is a
    transient sub for the duration of `init` while the official update goes
    through the change detector.
- **RF2 latches:** bit 29 dispatched this cycle; bit 30 one-shot `hen0`
  pending, armed at `0x95F9C` inside the tick state machine at `0x95EA0` when
  a routine completes (which also writes `+0xEE := 2` and RF0 bit 2), consumed
  at `0x8C628..0x8C662` by playing `hen0` through slot `+0x298` unless `+0xEE`
  is in {3, 4, 5}; bit 31 initialized latch.
- **Tick state machine** at RVA `0x95EA0(entity)`: gated by RF0 bit 5, keyed
  on `Status - 2` through the byte map at RVA `0x95FF0` (82 entries) and the
  two-entry jump table at `0x95FE8`; both blocks converge on the routine-end
  logic above.
- **Rebuild gate** at RVA `0x95DB0(entity)`: `test byte [esi+0x12c],1 / je
  skip / call 0x92910 / call 0x8F750 / and al,0xFE`, that is `if (RF3 & 1) {
  rebuild; update; RF3 &= ~1 }`. The update routine `0x8F750..0x926C7` has no
  status test of its own. Three callers, all per-entity loops (`0x95A43`,
  `0x95A81` via `0x95AEA` which first sets RF3 bit 29, `0x95B9A`). Bit 0 is
  set by `or [eax+0x12c],1` at `0xB9121` (function `0xB90F0`, actor looked up
  by index in the global table at `0x10480B30`, reached only through the
  stdcall thunk table near `0xBC6AF`). Bits 0 and 23 are cleared together at
  `0x87DC0` (`and 0xFFF7FFFF`), bit 23 alone at `0xBD378` (`and 0xFFFEFFFF`).
  At frame granularity RF3 bit 0 was never seen set: it is set and consumed
  within one flush.

### Actor vtable slots (CXiSkeletonActor, vtable at RVA `0x330F40`)

| Slot | RVA | Role |
| --- | --- | --- |
| `+0xA4` (41) | `0xA4B20` | read the sub byte at actor `+0x8A9` |
| `+0xA8` (42) | `0xA4B30` | write it (`mov [ecx+0x8a9],al; ret 4`) |
| `+0x298` (163) | `0xCEE50` | play a named routine |
| `+0x29C` (164) | `0x84CD0` | play a named routine |
| `+0x2A0` | | check (actor, fourcc) |

Both play slots resolve the FourCC through the shared helper `0xCE490`
(vtable slot `+0x1EC`), which walks the actor's loaded ANI clip list
(`[esi+8]` = next) and special-cases `init` at its tail, then start it via the
global at `0x72FC0`. "Set animationsub" is therefore a plain byte store.

### Post-init hook and the `!` clip family

`0xCF070(actor, actor)`: if the word at actor `+0xB2` is non-zero, `0xCF0C0`
runs: when bit 7 of the byte at actor `+0x840` is set, clear it and try
`!kl1`, `!kl2`, `!kl3` through `0xCEF10`; otherwise latch bit 7 and call
`0xCEF10` with the sentinel `!in?`, which expands to the candidate list at RVA
`0x330F28` = `!in1`, `!in2`, `!in3`. The weapon path near `0xD608x` tries
`!f01` and `!f02`, gated on bit 13 of `+0x840`. These are the only `!`-prefixed
literals in the binary, so the prefix reads as a convention for internal or
secondary clips. Not verified against game data: the PhoenixXI install ships
only `FFXiMain.dll` plus FTABLE/VTABLE, no ANI or model DATs.

### How the 0x0E handler stores `animationsub`

The handler at RVA `0x9BCF7` maintains both RF1 copies with convergent
XOR-diff writes (as it does for the status bits in RF0). At `0x9BE55..0x9BE63`:
`if ((status_byte & 1) || (dword at pkt+0x28 & 0x4000000))` take variant B at
`0x9BE88`, `sub << 1; xor RF1; and 0xE` (a full 3-bit write into bits 1-3, the
copy the dispatcher reads); else variant A at `0x9BE6D`, `sub << 13; xor RF1;
and 0x6000` (a 2-bit write into bits 13-14, consumed elsewhere, for example
`0x7A4C1` and `0x7BDAD` test bit 13). Common tail at `0x9BEA5`: `RF1 ^= diff`.
So an odd status, or bit 2 of the byte at `pkt+0x2B` (the byte after
`animationsub`), selects the low copy; an even status without that bit selects
the high copy. A sub of 8 seen on one unwatched entity does not fit three bits
and the `shl 1 / and 0xE` drops it to 0.

## The spawn flag rides through unmasked

A worm first seen already underground (RF0 `0x00C06000`: status 3, no actor)
carried RF1 `0x0200055A`, sub 5 in bits 1-3, that is the raw wire value
`1 | 0x04` with the server's spawn flag intact, and RF4 bit 7 set. Its spawn
packet had mask `0x57` sub 5; a later mask `0x30` packet carried sub 1. With
the table above, sub 5 resolves to `ini1` and sub 4 to `init`: the mod-4 wrap
is what absorbs the spawn flag, and the normalizer's steady states (sub 4 with
nibble 0, sub 0 with nibble 4) are "zero with and without the spawn flag". An
eight-entry table indexed by the raw 3-bit value reproduces retail exactly.

RF3 bit 23 (`spop`) was 0 on all watched worms; a worm spawning into view was
not observed, so the zone-in pop effect case is untested.

## A burrow cycle, field by field

- **Dig** (0x0E sub 1 while visible): RF1 bits 1-3 := 1, RF1 bit 11
  (`0x800`) cleared, RF1 and RF2 bit 4 set, RF4 bit 7 set. RF2 goes
  `0xA0020001` to `0xA0020011`. About 3 s later status flips to 3
  (INVISIBLE): RF1 |= `0x100`, sub still 1.
- **Pop** (0x0E sub 1 on a hidden entity): the actor is destroyed and
  recreated, and the create path resets the sub in RF1 to 0. RF2 drops to
  `0x00020001` (bits 29 and 31 cleared, matching the `and 0x5FFFFFFF` at
  `0x90CAD`) and returns to `0xA0020001` two frames later when `init`
  dispatches. RF1 on the fresh actor goes `..112` (create) to `..180` to
  `..800` at lock start. ActionTimer2 read a stale value before the pop and
  reset at lock start.
- **Sub 0** (0x0E sub 0 after the pop): only RF4 bit 7 clears; the sub in RF1
  is already 0. RF4 bit 7 is a "sub != 0" latch: set on the sub 1 packet,
  survives destroy and create, cleared on the sub 0 packet (reconfirmed nine
  times).
- **RF1 bits 13-14** never changed on any worm, so the second copy is not
  written for this entity kind.
- **Hide bits:** a live entity hidden by INVISIBLE shows RF0 `0x00C16000`;
  one destroyed by despawn, or first seen already hidden, shows `0x00406000`
  or `0x00C06000`. Bit 16 (`0x10000`) is set only by the destroy-from-live
  path, and bit 23 (`0x800000`) only by INVISIBLE.

## `animationsub -> 0` during a routine is a no-op

Across the three worm runs, 13 sub 0 packets arrived while a pop routine was
still locked. Twelve did nothing: the lock ran its natural 94 frames. The one
early cut (71 frames) landed in the same frame another worm's dig routine
started, and an earlier run's single cut coincided the same way. Scheduler
nodes are a shared pool (the same node address served two worms), so the rare
early release is cross-entity contention when another mob dispatches in the
same frame, not a cancel semantics of sub 0. A cancel would only be possible
if the sub in RF1 were still 1 at arrival, and the create path has already
zeroed it by then.

## Routine record format

Both burrow routine records were read at the moment of dispatch through the
first scheduler node's `+0x114` and `+0x118` pointers. A parsed record is a
flat stage stream: each stage begins with a dword whose low byte is the stage
type and whose high byte is the stage length in dwords, header included
(holds on all 12 stage boundaries in the two dumps). Types seen:

| Type | Length (dwords) | Payload |
| --- | --- | --- |
| `0x01` | 2 | record header |
| `0x5F` | 4 | `+8` = FourCC of the sibling routine it stops |
| `0x1F`, `0x07` | 3 | not decoded here |
| `0x05` | 10 | motion: `+4` timing word (`0x00640010` dig, `0x00960002` pop), `+8` clip FourCC, `+0x10` and `+0x14` = 1.0f, `+0x1C` = `0x00010028` |
| `0x0A` | 8 | sound: `+4` = 0 (dig) or 5 (pop), `+8` = sound id as four ASCII digits |
| `0x02` | 4 | VFX generator: `+4` timing (high u16 plausibly the start frame), `+8` instance FourCC, `+0xC` pointer to the generator object |
| `0x29` | 4 | appears mid-stream in the pop routine, so not a terminator |

Decoded: dig `ini1` = stop `init`, motion `sp1?`, sound 7025, generators
`kak0`, `mok0`, `mok1`, `dis0`; pop `init` = stop `ini1`, sound 7024, motion
`sp0?`, generators `mok1`, `kak1`, plus the `0x29` stage and a VFX stage. Both
match the model DAT byte for byte. The FourCC at `+0x10` of a record is the
payload of its `0x5F` stage (the sibling's name), not the record's own name.
The routine carries the sound id as ASCII digits; the client's path template
`se%3.3u/se%6.6u.spw` resolves it.

## Lock lengths

| Routine | Frames | Seconds |
| --- | --- | --- |
| worm dig `ini1` (6 observed) | 56 | 1.94 |
| worm pop `init` (7 observed) | 94 | 3.24 |
| worm death | 19-22 | ~0.7 |
| worm hit taken or swing | 15-42 | 0.5-1.4 |
| hare melee round `atk0` | 24-26 | ~0.85 |
| hare damage reaction | 15 | ~0.5 |
| hare TP move (category 11) | 40-44 | 1.4-1.5 |
| hare death | 53 | ~1.8 |
| hare engage stance transition | 14 | ~0.5 |

ActionTimer1 is a count, not a flag: it read 2 when a hit reaction overlapped
a swing and 3 during a TP move, consistent with the `inc word [ecx+0x11C]` at
RVA `0xA4150`. Death length differs per model, as expected for a DAT routine.

## Engage, hits, death, despawn (Carrion Worm)

- **Engage:** 0x0E mask `0x06` with animation byte 1 sets StatusServer 0 to 1
  and RF3 bit 28; Status syncs seven frames later; bit 28 clears on a later
  mask `0x03` packet. The actor colour word at `+0x78` went `C0808061` to
  `C0804141`.
- **Hits and swings:** every hit taken and every swing is a short scheduler
  routine on the worm's actor, with actor `+0x84` (ActType) stepping 4, 2, 3,
  `0xD`. Records crawled during those locks carry FourCC families never seen
  during burrow: `dam2`/`dam3`/`dam4` in a record of `0x0A` sound stages right
  after the first player hit, `atk2`/`atk3`/`atk4`, `at2?` with `skaz` and
  `dada`, `hit1`, `main`. Melee reactions and swings use the same mechanism
  as burrow: named DAT routines resolved against the model DAT through
  `0xCE490` and pushed as scheduler nodes, triggered by the action packet
  rather than by 0x0E.
- **Death:** 0x0E mask `0x06` with animation byte 3 sets StatusServer 1 to 3,
  HP to 0 and RF3 bit 28; Status follows eight frames later with a 22-frame
  lock.
- **Despawn:** 0x0E status 2, mask `0x30`, size 72: UpdateMask `0x0F` to
  `0x00`, actor destroyed, RF0 `0x00402200` to `0x00406000` (no INVISIBLE
  bits), RF1 bit 12 set then bit 11 cleared.

## Action packets drive routines (Forest Hares)

98 decoded 0x28 packets over two hares fought to death.

- **Melee round, category 1:** the 32 bits at bit 86 (the field LSB packs as
  `actionid`/`recast` and now names `m_uCmdArg` in `four_cc.h`) read as the
  FourCC `atk0` (`0x306B7461`) from both hares and the player. The result is
  one routine per round, locked 24-26 frames, starting the frame after the
  packet, hit or miss alike. Crawls during those locks show the hare's swing
  clips `at00`, `at10`, `at20`, `at21`, never the same one twice in a row, so
  the variant is picked client-side. The result `anim` field is 0 for mobs
  (1 for a player's second double-attack swing).
- **Per-target result fields** (the addon read `resolution | kind << 3` as one
  5-bit value): 8 hit, 9 miss, 24 weapon skill or TP move hit; `info` 32
  damage, 64 and 96 critical variants, 34 and 66 with message 67 = critical;
  messages 1 hit, 15 miss, 43 "readies", 185 weapon skill or TP damage.
- **TP move:** two packets about 15 frames apart. Category 7 carries the
  FourCC `cate` (`0x65746163`), target = the mob itself, param = mob skill id
  259 (Foot Kick), message 43; it produces no lock, because the DAT routine
  `cate` (call `nerm` plus cast) has no `0x59` AnimationLock stage. Category
  11 (param 259, resolution 24, `anim` 3, message 185) starts two routines at
  once and a third two frames later, total lock 40-44 frames; a motion node
  carried `sp10` and a crawl showed `wz60`. The `sp` clip family is the
  per-model special set (worm `sp0?`/`sp1?` = pop/dig, hare `sp1?` = Foot
  Kick); the mob skill's `anim` value (server side: LSB `mob_skills.animation`)
  is what selects it. A player weapon skill is category 7 (`cate`, param = WS
  id) followed by category 3 (param 1, resolution 24, `anim` 16, `info` 97).
- **Target side:** being hit runs a 15-frame damage-reaction routine on the
  target, overlapping freely with the target's own swing. Clips seen around
  those frames: `btl0`/`btl1` (battle stance), `swy1`/`swy2`/`swy3` (sway or
  knockback, after a two-hit round), `hit6`, `dfi6`/`dbi6`. Engage: 0x0E mask
  `0x06` animation byte 1, StatusServer 0 to 1, Status synced seven frames
  later, then about nine frames on a 14-frame lock with no packet behind it
  (the idle-to-battle-stance transition, `btl` clips). A weapon-skill or
  TP-move kill overlaps the reaction and death routines (ActionTimer1 0 to 2);
  Status goes 1 to 3 when the packet's `anim` 3 syncs.

## Leaving and re-entering view replays `init`

Three hares went through UpdateMask `0x0F` to `0x00`, RF0 to `0x00406000` and
actor destroyed on leaving range, then UpdateMask back to `0x0F`, RF0
`0x00402200`, a new actor, and RF2 `0xA0020001` to `0x00020001` to
`0xA0020001` two frames later on returning. This is the same create path as
the worm's pop: a view-range respawn is a destroy and create and replays
`init`.

## Open questions

- The state dispatch function treats `+0xEE` as a status byte (gate "not in
  {3, 4, 5}", written to 2 on routine completion), but the same offset reads
  as the entity Type elsewhere (worm Type = 2). Settle which byte is meant
  before building a raw-state machine on it.
- The sub block's restore of the old sub after `init`.
- Which swing or reaction name is picked per action (attack id, hit or miss,
  damage bracket); candidates are the weapon-state block (`wep0`..`wep8` via
  `0xD60F0`) and the switch machine at `0xD6B43`.
- The meanings of `inte`, `dfi6`/`dbi6`, and whether `wz60` is a generator.
- How a mob skill's `anim` value maps to a model's `sp` clip.
