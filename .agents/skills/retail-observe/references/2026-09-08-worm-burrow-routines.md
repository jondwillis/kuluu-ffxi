# Worm burrow routines + animation dispatch (retail FFXI, observed on HorizonXI + static)

Runtime: 2026-09-08 and 2026-09-09, Carrion Worms and Forest Hares in the retail client,
observed with an Ashita capture addon (packet + per-actor memory probe; the addon and its
raw logs are not in this tree). Static: 2026-09-08, gate probes and full-function dumps
against retail `FFXiMain.dll`. Finding ids (F36..F53) are this record's own numbering; the
earlier ids they reference (F13..F35) come from the same capture series and are quoted where
they matter. Kuluu code cites entries here by id.

## Retail runtime findings (Phase L, closed 2026-09-08)

- **F39 [local] 2026-09-08 (wormwatch_20260908_182632.log, v0.4, Carrion Worm idx
  102):** RF1 decode across a full cycle: dig sub=1 -> subA=1 (bits 1-3), RF1 bit
  0x800 cleared, RF1/RF2 bit 4 set, RF4 bit 7 set. INVISIBLE: RF1 |= 0x100, subA
  still 1. Pop packet (sub=1): new actor and **subA reset to 0**; RF1 returns to
  0x800 at lock start 2 frames later. sub=0 packet: only RF4 bit 7 clears; subA
  already 0; lock runs natural 94 frames. **RF3 never changed** (bit 0 / bit 23 not
  observable at frame granularity: set and consumed within one flush). **subB
  (bits 13-15) never changed**: F30's "second copy" is not a sub copy for this mob;
  re-read 0x9BE6D.
- **F40 [local] (H4, explains F13 vs F38):** sub=0 mid-routine is a no-op when the
  create path has already zeroed subA (this run, F13). It cancels the lock only
  when subA is still 1 at arrival (worm 100 in F38), i.e. a real 1->0 transition on
  a live actor dispatches a routine change (candidate: F29's init->ini0 rewrite).
  Race between create-path reset and packet order. Test: Lock ALL, several cycles,
  correlate `subA=1 -> subA=0` ENT lines with lock cuts.
- **F41 [local]:** RF4 (+0x130) bit 7 = "sub != 0" latch: set on the sub=1 packet,
  survives destroy/create, cleared on the sub=0 packet.
- **F42 [local] 2026-09-08 (wormwatch_20260908_182935.log, 3 Carrion Worms, 7 pop
  locks, 8 sub=0 packets; F38/F40 resolved, H4 dead):** subA is 0 at every sub=0
  arrival (create path resets it, F39); 5 of 6 sub=0 packets during an active pop
  lock did nothing (locks ran 94 ticks). The one cut (idx 100, 71 ticks, f8165)
  landed in the same frame worm 102's dig lock started; the F38 cut (f3362)
  likewise coincided with worm 102's dig start. Scheduler nodes are a shared pool
  (same node address used by both worms). Conclusion: sub=0 mid-routine is a
  no-op; the rare early lock release is cross-entity contention when another mob
  starts a routine in the same frame. Retail quirk, **no Kuluu action** - Kuluu's
  driver has no shared scheduler pool to contend over (validates `8fe705f`'s
  "sub->0 does not cancel" semantics). Not pursued further.

Also in the F42 log: death path = StatusServer/Status -> 3 from the animation byte,
HP -> 0, RF3 bit 28 set, short 19-22 tick lock; post-death disappear shows RF0
0x00406000 (status=3, actor=0) **without** the 0x00C16000 bits INVISIBLE sets, so
those extra bits (0x800000|0x10000) distinguish hide-from-INVISIBLE from
hide-from-despawn.

## Static verification (p3 gates + full-function dumps, 2026-09-08)

- **F36 [local] 2026-09-08 (G1+G2):** RF3 (+0x12C) bit 0 = "rebuild pending". No
  status test inside the update routine 0x8F750..0x926C7; the gate is caller func
  @0x95DB0(ent): `test byte [esi+0x12c],1 / je skip / call 0x92910 / call 0x8F750
  / and al,0xFE` - i.e. `if (RF3 & 1) { rebuild; update; RF3 &= ~1 }`. Three call
  sites, all in per-entity loops (@0x95A43 loop @0x95A58; @0x95A81 path via
  @0x95AEA which first writes `RF3 |= 0x20000000`; @0x95B9A loop @0x95BAB). G1 RMW
  list: bit-0 SETTER = `or [eax+0x12c],1` @0xB9121 (func 0xB90F0, actor from global
  table @0x10480B30 by index; reached only via the stdcall thunk table ~0xBC6AF -
  callback registration); clear bit0+bit23 helper @0x87DC0 (`and [ecx+0x12c],
  0xFFF7FFFF`); clear bit23 only @0xBD378 (`and 0xFFFEFFFF`); two `and
  [esi+0x12c],ebx` in the caller loops @0x95A81/0x95C97.
- **F37 [local] 2026-09-08 (G3 + full dumps):** THE sub->clip dispatch, fully
  decoded:
  - Sub->fourcc table @RVA 0x35AF60 (.data): **inline fourcc array** (not
    pointers), index = RF1 bits 1-3 (`shr eax,1; and al,7` @0x8C5C8 / @0x8F091):
    `[init, ini1, ini2, ini3, init, ini1, ini2, ini3]` - sub 4..7 wraps mod-4.
    Verified byte-for-byte against on-disk .data (POL1 packing only affects .text).
  - Normalizer @0x8EF70(ent): live sub = RF1 bits 1-3; keeps a mirror in RF1 bits
    4-6 (`RF1 := low_nibble | (sub<<3)`); syncs actor+0x8A9 via setSub (+0xa8);
    routes to the change detector when not in steady state (steady: sub=4 &
    nibble=0, or sub=0 & nibble=4).
  - Change detector/applier @0x8EFD5 - the real dispatch: compares RF1 bits 1-3 vs
    actor+0x8A9 (getSub, +0xa4) and the mirror nibble; on change: `setSub(new)`
    then plays `table[sub]` via vtable **+0x29C** and/or **+0x298**, gated by RF0
    bit 9 (+RF4 bit 2 if RF0 bit 13 clear, @0x8F0B2..D8), a check call to
    +0x2A0(actor, fourcc) (@0x8F097-0x8F0A5), and (sub>=4 path) requires RF2 bit
    29 set.
  - State dispatch func @0x8C490(ent): prologue - when RF2 bit 31 is clear: status
    byte +0xEE not in {3,4,5} && predicate 0xD12E0(actor) -> call 0xCF110(ent,0),
    set RF2 bit 31 ("initialized" latch). Entry skip when RF2 bit 29 "dispatched
    this cycle" (set @0x8C61A after processing; cleared with bit 30 by `and
    [esi+0x128],0x5FFFFFFF` @0x90CAD in a destroy/despawn-ish path). Jump table on
    **[esi+0x170] - 6** (byte map @RVA 0x8C680, jump table @0x8C670):
    - raw states {6, 50, 56} (idx 0/44/50) -> fishing helper 0x8C6D0(ent, state):
      plays **'fsh0'..'fsh3'** via +0x29c.
    - raw state 34 (idx 28) -> plays 'init' then **'inte'** (`push 0x65746e69`,
      bytes i-n-t-e - verified literal, unexplained; record as observed), both via
      +0x298; clears actor+0x7D8 to spaces.
    - raw states 64..83 (idx 58-77) -> 0xD60D0: loops 9x calling 0xD60F0 which
      builds fourcc **'wep'+digit** ('wep0'..'wep8') = weapon-attack clips.
    - all other states (<6, >=84, and the rest) -> sub-dispatch block @0x8C5A8:
      getSub->save; setSub(new from RF1 bits 1-3); play 'init' via +0x298;
      post-init hook 0xCF070(ent, actor); clear actor+0x7D8 to spaces; **then
      setSub(old) again** - unexplained (best hypothesis: transient sub for the
      duration of 'init'/post-init processing while the official update happens via
      @0x8EFD5). Open question.
  - RF2 (+0x128): bit 29 = "dispatched this cycle"; bit 30 = one-shot **'hen0'**
    pending - armed @0x95F9C (in tick SM func @0x95EA0) when a routine completes,
    which also sets status byte +0xEE := 2 and RF0 bit 2; consumed+cleared
    @0x8C628..0x8C662 after playing 'hen0' via +0x298 (skipped if status in
    {3,4,5}); bit 31 = initialized latch.
  - Second state machine @0x95EA0(ent) ("tick/advance"): gated by RF0 bit 5; keyed
    on [esi+0x170]-2, byte map @RVA 0x95FF0 (82 entries), jump table @0x95FE8 =
    [0x95ED5, 0x95EDE]; both blocks converge to the routine-end logic above.
- **F43 [local] 2026-09-08 (G4; resolves F30's open question - where the previous
  sub lives):** handler @0x9BCF7 maintains both RF1 copies itself via convergent
  XOR-diff writes (like the status bits in RF0). Branch condition @0x9BE55-0x9BE63:
  `if ((status_byte & 1) || ([pkt+0x28] dword & 0x4000000))` -> **variant B
  @0x9BE88**: `sub<<1; xor RF1; and 0xE` (full 3-bit write into bits 1-3 = the copy
  the dispatcher reads). Else **variant A @0x9BE6D**: `sub<<13; xor RF1; and
  0x6000` (2-bit write into bits 13-14, consumed elsewhere e.g. funcs 0x7A4C1 /
  0x7BDAD which test bit 13). Common tail @0x9BEA5: `RF1 ^= diff`. So: odd status
  or the +0x2B flag -> low copy; even status (2,4) without it -> high copy. Note:
  the tested bit is bit 26 of the dword at pkt+0x28 = **bit 2 of byte pkt+0x2B**
  (the byte after animationsub), not a bit of the sub byte itself.
- **F44 [local] 2026-09-08:** vtable slots on CXiSkeletonActor (vtable @RVA
  0x330F40): +0xa4 (slot 41) @0xA4B20 = getter of byte actor+**0x8A9**; +0xa8
  (slot 42) @0xA4B30 = setter of the same byte (`mov [ecx+0x8a9],al; ret 4`) - so
  "set animationsub" is just a plain byte store. +0x298 (slot 163) @0xCEE50 and
  +0x29c (slot 164) @0x84CD0 = named-animation play slots: both resolve
  fourcc->entry via shared helper 0xCE490 / vtable slot +0x1EC, then start it
  (0x72FC0 global). +0x2A0 = a check method taking (actor, fourcc). Resolver
  0xCE490 walks the actor's loaded ANI clip linked list ([esi+8]=next) and
  special-cases 'init' at its tail.
- **F45 [local] 2026-09-08:** post-init hook family: 0xCF070(actor,actor): if word
  actor+0xB2 != 0 -> 0xCF0C0: if bit 7 of byte actor+**0x840** set -> clear it, try
  playing **'!kl1','!kl2','!kl3'** via 0xCEF10. Else (path B): latch bit 7 of
  +0x840 and call 0xCEF10 with sentinel **'!in?'** (`push 0x3f6e6921`), which in
  0xCEF10 expands to trying the candidate list @RVA 0x330F28 = ['!in1','!in2',
  '!in3']. Weapon path ~0xD608x tries '!f01'/'!f02' gated on bit 13 of +0x840. The
  **'!' prefix is a systematic convention** (only these literals in the whole
  binary: !f01, !f02, !in?, !kl1-3). Best inference: literal clip names with '!'
  marking internal/secondary clips. Open question - cannot verify against game data
  (PhoenixXI install ships FFXiMain.dll + FTABLE/VTABLE.DAT only, no ANI/DAT).

## Retail runtime findings, third session (wormwatch_20260908_231759.log, 2026-09-08)

Three Carrion Worms (idx 100/101/102) locked for ~4 minutes: 6 digs, 7 pops, then the player
engaged worm 102, killed it and watched the despawn. New here: the parsed routine record format
(and a correction to F22), proof that the spawn flag rides through unmasked, and the first
non-burrow routines (damage / attack / hit families).

- **F46 [local] 2026-09-08 (wormwatch_20260908_231759.log, Carrion Worms idx 100/101/102, v0.4;
  ROUTINE RECORD FORMAT + correction to F22):** the crawl caught both burrow routine records at the
  moment of dispatch, through the first scheduler node's +0x114/+0x118 pointers (dig: node
  0x2911A174 -> 0x261280A0 / 0x261280F8 at f6473, worm 101; pop: node 0x2911BF34 -> 0x261282F0 at
  f4382, worm 100). A parsed routine record is a flat **stage stream**: each stage begins with a dword
  whose low byte is the stage type and whose high byte is the stage length in dwords, header
  included (holds on all 12 consecutive stage boundaries in the two dumps). Types seen: 0x01 record
  header (2 dw); **0x5F sibling cross-reference** (4 dw, payload +8 = fourcc of the *other* routine);
  0x1F and 0x07 (3 dw each, unknown); **0x05 motion** (10 dw: +4 timing word 0x00640010 dig /
  0x00960002 pop, +8 clip fourcc, +0x10 and +0x14 = 1.0f, +0x1C = 0x00010028); **0x0A sound** (8 dw:
  +4 = 0 dig / 5 pop, +8 = sound id as four ASCII digits); **0x02 VFX generator** (4 dw: +4 timing,
  high u16 plausibly the start frame, +8 instance fourcc, +0xC pointer to the generator object);
  0x29 (4 dw, unknown; appears mid-stream in the pop routine, so not a terminator). Decoded:
  **dig = {xref 'init', motion sp1?, sound 7025, kak0@0x36, mok0@0x37, mok1@0x12, dis0@0x19}** and
  **pop = {..., sound 7024, mok1@0x6C, motion sp0?, kak1@0x08, 0x29, VFX@0x5A, ...}**. Both match the
  earlier DAT dump byte for byte (ini1 = sp1? + kak0/mok0/mok1/dis0 + 7025; init = sp0? +
  dis0/mok1/kak1/kak0/mok0 + 7024). **Correction to F22:** the fourcc at +0x10 that F22 took for the
  record's own name ('ini1') is the payload of the 0x5F cross-reference stage, i.e. the sibling's
  name. F22's record {dis0, 0x307, 7024, mok1} was therefore `init` (pop), not `ini1`, and F22's
  "the earlier dump's routine contents are swapped" is withdrawn: dig = `ini1` = 7025, pop = `init`
  = 7024. Closes the earlier sound-attribution item and the sound half of Q5 (the routine
  carries the SE id as ASCII digits; the path template `se%3.3u/se%6.6u.spw` from F25 is the resolver).
  Raw dumps are not in tree.
- **F47 [local] 2026-09-08 (spawn flag 0x04 is NOT masked by the client):** worm 100 was already
  underground when locked (SNAP f328: RF0 0x00C06000 = status 3, no actor) and carried RF1
  0x0200055A = **subA 5**, the raw wire value 1|0x04 stored unmasked in bits 1-3, RF4 bit 7 set
  (unwatched idx 676 spawn packet: mask 0x57 sub=5; a later mask 0x30 packet carried sub=1). With
  F37's table [init, ini1, ini2, ini3, init, ini1, ini2, ini3], sub 5 resolves to 'ini1' and sub 4 to
  'init': **the mod-4 wrap is what absorbs the spawn flag**, and the normalizer's steady states
  (sub=4 & nibble=0, sub=0 & nibble=4) are "zero with / without the spawn flag". Kuluu's
  `sub & !0b100` is equivalent for sub<4; indexing an 8-entry table with the raw 3-bit value matches
  retail exactly. On worm 100's pop (f2462) the create path reset subA 5 -> 0 as in F39 and 'init'
  ran the full 94 frames (ActionTimer2 read a stale 17096 before the pop; reset to 1798 at lock
  start). RF0 before the pop was 0x00C06000 vs 0x00C16000 after a live destroy: bit 16 (0x10000) is
  set by the destroy-from-live path and absent when the entity was first seen already hidden. RF3
  bit 23 ('spop') was 0 on all three worms; the zone-in case  is still
  untested because no watched worm spawned into view.
- **F48 [local] 2026-09-08 (lock statistics; F42 confirmed):** 6 dig locks all **56 wormwatch frames
  (1.94 s)**, 7 pop locks all **94 frames (3.24 s)**; one wormwatch frame is ~34.5 ms (client ~29 fps),
  so F42's "ticks" are these frames. 7 sub=0 packets arrived mid-pop (f2172, f2318, f2534, f4451,
  f4534, f4550, f6730): **7/7 no-ops**, every lock ran to 94. No early cut this session (the worms
  dug and popped on different frames). Cumulative over the three v0.4 logs: 12 of 13 mid-pop sub=0
  packets did nothing; the one cut coincided with another worm's dispatch in the same frame (F42).
  RF4 bit 7 latch (F41) reconfirmed 9 times. RF2: 0xA0020001 idle -> 0xA0020011 (bit 4) on dig;
  destroy -> create drops it to 0x00020001 (bits 29/31 cleared, matches `and 0x5FFFFFFF` @0x90CAD)
  and it returns to 0xA0020001 two frames later when 'init' dispatches (bit 29 dispatched-this-cycle,
  bit 31 initialized latch). RF1 on a fresh actor: 0x...112 -> 0x...180 (create) -> 0x...800
  (dispatch), as in F39.
- **F49 [local] 2026-09-08 (first non-burrow routines: engage, hits, death, despawn; worm 102,
  f5196-f6764):**
  - Engage: 0x0E mask 0x06 anim=1 (f5203) -> StatusServer 0->1 (F30's animation-byte path), HP
    100->24, RF3 bit 28 set; Status([esi+0x170]) 0->1 synced 7 frames later; RF3 bit 28 cleared on
    a later mask 0x03 packet (f5341). Actor colour word +0x78 C0808061 -> C0804141 on engage.
  - Every hit taken and every swing is a short scheduler routine on the worm's actor: locks of
    15-42 frames (0.5-1.4 s), actor +0x84 (ActType) stepping 4 -> 2 -> 3 -> 0xD. **ActionTimer1 is a
    count, not a bool**: it read 2 at f6218 when two routines overlapped (consistent with `inc word
    [ecx+0x11C]` @0xA4150, F34).
  - Records crawled from those nodes carry fourcc families never seen during burrow: **'dam2'/'dam3'/
    'dam4'** in a record made of 0x080A sound stages (f5207, right after the first player hit = damage
    reaction), **'atk2'/'atk3'/'atk4'** (f5220), **'at2?'** with 'skaz'/'dada' (f5196), **'hit1'**
    (f5897), 'main'. So melee reactions and swings use the same mechanism as burrow: named DAT
    routines resolved against the model DAT (0xCE490 / slot +0x400) and pushed as scheduler nodes,
    triggered from the action packet rather than from 0x0E. Which name is picked per action (attack
    id, hit vs miss, damage bracket) is the next thing to read; candidates are F37's weapon-state
    block ('wep0'..'wep8' via 0xD60F0) and the 0xD6B43 switch machine (F29). Raw excerpts:
    (raw dumps not in tree).
  - Death: 0x0E mask 0x06 anim=3 (f6232) -> StatusServer 1->3, HP->0, RF3 bit 28 set; Status 1->3 at
    f6240 with a 22-frame lock. Despawn: 0x0E status=2 mask 0x30 size 72 (f6762) -> UpdateMask
    0x0F->0x00, actor destroyed, RF0 0x00402200 -> 0x00406000 (no INVISIBLE bits 16/23), RF1 bit 12
    set then bit 11 cleared. Matches the F42 death note.
  - Open: F37 calls +0xEE a "status byte" (gate `not in {3,4,5}`, and `:= 2` on routine completion),
    but +0xEE is `Type` per F6/F7/F28 (worm Type = 2). Re-read 0x8C490/0x95F9C to settle which byte
    is meant before building the raw-state machine on it.

## Fourth session: Forest Hares, melee + TP moves (wormwatch_20260909_000019.log, 2026-09-09)

wormwatch v0.5 (0x28 decode + auto-lock). Two hares fought to death by the player, 98 action
packets, 6 hares locked. First direct evidence of how action packets drive mob routines.

- **F50 [local] 2026-09-09 (wormwatch_20260909_000019.log, v0.5, Forest Hares idx 94/97 + 4 more
  auto-locked; MELEE = 'atk0' by name):** 98 decoded 0x28 packets. Every melee round (category 1),
  from both hares and from the player, carries **0x306B7461 in the 32 bits at bit 86**, i.e. the
  fields LSB packs as actionid=29793 / recast=12395 read as the fourcc **'atk0'** (confirmed
  2026-09-09 by SE's code, F56: it is one 32-bit field, `m_uCmdArg`, and LSB's `four_cc.h` now names it). WS/mobskill
  start (category 7) carries 0x65746163 = **'cate'** (LSB 24931 / 25972). These are LSB constants
  (grep the server for 29793 and 24931), presumably copied from retail captures; whether the client
  reads that dword as the routine name or derives it from the category is not settled, but the
  actor-side result is the same every time: **one routine per melee round, lock 24-26 frames
  (~0.85 s), starting the frame after the packet, hit or miss (react 8 vs 9 makes no difference to
  the attacker).** Crawls during those locks show the hare's attack motion clips **'at00', 'at10',
  'at20' and 'at21'** (the 'at?0'/'at2?' wildcard family), never the same one in sequence, so the
  variant is picked client-side; the packet's result `anim` field is 0 for mobs (players: 1 = the
  second swing of a double attack). Per-target result fields as decoded: react 8 hit / 9 miss / 24
  WS or TP-move hit; eff 32 damage, 64/96 crit-ish variants, 34/66 with msg 67 = critical; msg 1 hit,
  15 miss, 43 "readies", 185 WS/TP damage.
- **F51 [local] 2026-09-09 (TP moves):** the mob TP move is two packets ~15 frames apart: category 7
  ('cate', target = the mob itself, result param = mob skill id 259 = Foot Kick, msg 43) then
  category 11 (param 259, result react 24, **anim 3**, msg 185). The category-7 "readies" packet
  produces **no lock** on the mob (corrected 2026-09-09 by F55: it runs the DAT routine `cate` = call
  nerm + cast, which has no 0x59 AnimationLock stage; the node was not visible because the watched
  head node was occupied by the hit reaction). The category-11 packet starts **two routines at
  once** (ActionTimer1 jumps by 2, a third joins 2 frames later), total lock **40-44 frames
  (~1.4-1.5 s)**; a motion-task node carried **'sp10'** and a crawl showed 'wz60' (generator?). So the
  'sp' clip family is "special" per model (worm: sp0?/sp1? = dig/pop, hare: sp1? = Foot Kick), and
  the mob skill's `anim` value (3 here) is what the client turns into a routine/clip index; how 3
  maps to sp1? for this model is the remaining question (LSB `mob_skills.animation` is the server-side
  source of that number). Player WS: category 7 ('cate', param = WS id 1) then category 3 (param 1,
  react 24, anim 16, eff 97).
- **F52 [local] 2026-09-09 (target side):** being hit runs a **damage-reaction routine of 15 frames
  (~0.5 s)** on the target (f1987-f2002, f6156-f6171, f6478-f6493), overlapping freely with the
  target's own swing (ActionTimer1 counts both). Clips seen around those frames: **'btl0'/'btl1'**
  (battle stance), **'swy1'/'swy2'/'swy3'** (sway/knockback family, after the two-hit round at
  f6155), 'hit6', 'dfi6'/'dbi6' (recurring near reactions, meaning open). Engage: 0x0E mask 0x06
  anim=1 -> StatusServer 0->1, Status synced 7 frames later; ~9 frames after that a **14-frame lock**
  with no packet behind it (tentatively the idle->battle-stance transition, btl clips). Death: WS/TP
  kill -> reaction + death routines overlap (ActionTimer1 0->2 on hare 97), Status 1->3 when the
  packet's anim=3 syncs; hare 94's death lock ran 53 frames vs the worm's 22, so death length is
  per model, as expected for a DAT routine. Despawn 0x0E status=2 mask 0x30 = same flag pattern as
  F49.
- **F53 [local] 2026-09-09 (view-range despawn/respawn):** hares 94, 99 and 190 each went through
  UpdateMask 0x0F->0x00, RF0 -> 0x00406000, actor destroyed, then (when back in range) UpdateMask
  0x00->0x0F, RF0 -> 0x00402200, new actor, RF2 0xA0020001 -> 0x00020001 -> 0xA0020001 two frames
  later. Identical to the pop-up create path: **leaving and re-entering view is a destroy/create and
  replays 'init'.** Also seen unwatched: idx 110 with sub=8 (does not fit RF1's 3 bits; the handler's
  `shl 1 / and 0xE` drops it to 0) and idx 966 status=3 sub=1 mask 0x3F.
