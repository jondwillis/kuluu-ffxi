# Event opcode no-skip pass — 2026-10-28

## Method and scope

Static only, per vendor/rea/INSTALL.md: this install's `FFXiMain` image as generated C plus the two
event DAT corpora. No live session. Every handler below was read from the dispatch table of the
event opcode interpreter on this build (see Provenance), not from the atom0s/XiEvents docs; where a
doc-derived name already exists in kuluu (`SOUND_TYPE_*`) it is kept, and where none exists no name
was invented.

Corpus usage counts are from `cargo run -q --release -p ffxi-event --example zz-opcode-gap` (299 zone
event DATs, 186815 threads; saved under the session scratchpad as opcode-gap.txt). The gap list that
example prints was verified unreliable for "does kuluu have an arm" (visibility qualifiers and the
scene path defeat text matching); its counts are trustworthy and drove prioritisation. That arm
detection was rewritten on 2026-10-30 — four parsing bugs in the example, one of which reported every
constant as zero; the fresh honest gap list is in Provenance ("Gap-census tool fixed"), and every
gap-vs-armed claim quoted from this example before that date is superseded. That first fix was itself
incomplete the same day; the list to quote is the final **22-row** one in Provenance ("Gap-census detector
corrected again"), and the ledger's MATCH 184 / NO-ARM 20 / WIDTH-ONLY 15 remains the bucket carrying
verdicts.

## Volume staging: 0x69 SETSOUNDVOLUME / 0x6A CHANGESOUNDVOLUME

Retail has two volume opcodes, and kuluu skipped both by size despite the pair being live content
(224 uses across 48 zones; 426 uses across 55 zones). Both address retail's mixer through four
channel arms (one each for the low mask bits) plus a global whole-sound multiplier slot, and both
carry a channel mask whose low byte selects which arm runs:

- **Set (0x69)** carries an inline flag byte and one work operand. The flag decides a value that is
  exactly full scale when the flag reads zero and exactly silence otherwise — no ramp, applied once
  per selected channel. When bit four of the mask is set, retail additionally derives a reciprocal-
  style fixed-point form of that same 0-or-full value into the global sound multiplier slot; kuluu
  models that bit as scaling both of its buses (see delta).
- **Change (0x6A)** carries three work operands. The mask is the low byte of the third operand's
  resolved value; the target level is the first operand's value scaled by exactly one thousandth —
  retail's scale constant reads 1/1000 and its own docs call the unit "per-mille". Through each
  selected arm it also passes a second argument taken from the second work operand; no source here
  establishes what that mixer-side argument does (candidate: fade frames), so kuluu carries only
  the per-mille target (see Not established).

Kuluu's two buses are music and effects. Mapping used, stated as authored choice against retail's
four arms plus one global: mask bit two → the music bus; bits one, four and eight → the effects bus;
bit sixteen → both. Cutscene scope must restore both scales on session end exactly like the existing
0x5D duck (an event that silences BGM and ends without restoring would otherwise leak silence into
gameplay — this project has been burned by background-sound leaks before).

## Second door pair: 0x8E / 0x8F DOOR_OPEN2 / DOOR_CLOSE2

Identical to the already-implemented first pair except in one respect: they write the *second* door
status slot. Kuluu already carries both status constants; the arms were missing (763/113 uses per
corpus). Same event-entity owner, same gate on a render flag no tier names — which kuluu does not
model in VM state for the first pair either, and now documents at both.

## Map window: 0x8D OPEN_MAP_PROPS, 0xB8 MAP_ADD_MARK, 0x75 map sub-op family

- **OPEN_MAP_PROPS (0x8D)** shows or hides the prop-label layer of a zone's map window through the
  shared map-window helper, passing (mode 1 fixed, select, value) from two work operands. Corpus:
  524 uses / 23 zones. Kuluu has no prop-label layer on its Map screen yet; the state is carried to
  the host and stored so a later label layer consumes it (delta).
- **MAP_ADD_MARK (0xB8)** is the already-implemented marker opcode with one extra word operand
  inserted between the map id and the coordinates: five work operands (map id, a selector word that
  only reaches the shared window helper, x, y as milli units scaled by the same thousandths
  constant) and a sixteen-byte inline name starting two bytes later than on the plain marker
  opcode. Retail rewrites every underscore in the name to a space *before* handing it over — kuluu's
  existing rewrite is exactly that — and when its map-window state global reads closed, retail runs
  a helper-mode-four call instead of mode one and skips one per-marker setter (an icon-type setter);
  when no map window exists at all, retail advances past the whole record with nothing applied. The
  earlier hope recorded in this session's working notes that an all-underscore name triggers a
  clear-all path is **retracted**: reading the handler shows only the per-byte rewrite and the
  window-state branch; there is no all-underscore test. Corpus: 498 uses / 18 zones. Kuluu rides it
  onto the same marker cue as the plain opcode — every field the host acts on is identical, and the
  two words that reach only retail's window helper have no kuluu counterpart yet (delta).
- **The 0x75 family** is a window-singleton op read to its last instruction this pass; the earlier
  sketch here — hold-on-window-animation, case one closing the map through a work operand, case two
  skipping over the next instruction — is **retracted**: none of those three readings survives the
  dump. It first tests the singleton that counts open windows and if any window is open it raises
  the yield byte and returns without advancing; kuluu opens no such windows behind its back, so the
  gate never fires for it. It then switches on an inline selector byte. **Case 0** fetches one WORD
  operand through the code-redirect helper and hands the resolved value to that same singleton's
  present routine — which stores the word into its own window-id slot and runs its create/show
  chain, one branch issuing a client-to-server request (id 242) itself — then advances four bytes,
  unconditionally. **Case 1** fetches nothing and calls nothing: it just steps past opcode-and-
  selector, advancing two. **Case 2** re-bases exec six bytes backwards so its operand fetch reads
  the final WORD of the *previous* instruction, then hands (selector 2, that word) to a small sender
  which allocates client-to-server packet id 242 and sends it: when the send is accepted exec lands
  net two past this opcode; when it is refused exec is restored to this opcode's own start and the
  yield byte raised — it spins until some later pass gets the packet out. An unknown selector leaves
  exec untouched, which is exactly kuluu's halt. Every advance comes from the handler's four explicit
  pointer adds — its fetch helper never moves exec — so the three cases' widths are 4/2/2, and since
  the gate is unreachable for kuluu these widths *are* retail's observable outcome for case 1 and a
  successful case-2 send. The only divergence with a name is the refused-send spin, unobservable
  without a counterpart to request 242, plus the window-present effect itself: neither counterpart
  exists in kuluu (delta) — widths recorded, effects parsed-not-applied, no silent skip. Corpus:
  2219 uses / 57 zones.

## Per-actor flag writes: 0x92 / 0x94 flags-word bits, 0x81 visual-half byte

Three heavily-used per-actor staging opcodes (17150/221 zones, 13712/184, 4565/153) write render
state through the shared actor lookup plus its eligibility check (entity present and displayable —
an unresolvable or absent target is passed over):

- Two sibling opcodes **conditionally rewrite one bit** of the actor's second flags word — bit 17
  on the first, bit 18 counting from one, i.e. the two adjacent high bits of that dword — to an
  inline operand byte: the write happens only when the operand differs from the current bit (the
  handler xor-latches). Only bit zero of the operand matters.
- **0x81** writes a byte in the actor's visual half, 1 or 0 from whether its inline operand byte is
  non-zero; gated additionally on one display bit and the visual half being loaded.

No source available here names what any of these bits *do*: readers of the two flags-word bits are
not identifiable among the hundreds of uses of that dword without an observation pass, and reads of
the visual-half byte could not be located at all (writers only). Per this repo's precedent for 0x90's
un-named flag half, nothing is invented: kuluu parses each operand faithfully, resolves the actor
through the same lookup, publishes a per-actor render-flag cue carrying the bit index from the
image offsets, and no renderer owner applies it until retail observation names one (delta).

## Three-tick map/zone reload: 0x34 MAPLOAD / 0x35 MAPLOAD_KEEP

The pair is one state machine kept in two file-scope globals — shared across every event thread then
running — spread over three `EventIdle` deliveries, holding between each and advancing only on the
last. First delivery (idle): touch-refresh every entry of the scene entity array, bump a counter,
hold. Second delivery: set the counter to its armed value 300, hold. Third delivery (counter at the
armed value): if a reload-pending flag has meanwhile been set by streaming code, close the zone's
map-window data through the map singleton and skip the open step; otherwise run the teardown chain
and re-open the window for the id named in the first work operand; clear both globals, advance past
the three-byte instruction. The KEEP twin is identical except that its pending branch does nothing —
never closing anything (matching kuluu's existing comment). Corpus: 1166 uses / 121 zones and 902 /
116. Kuluu models the cadence as exactly this three-pass hold on the VM instance with a map-reload
cue (carrying the work-named id) published on the final pass; its entities refresh per frame anyway,
and nothing observable to kuluu ever sets retail's streaming-pending flag, so the pending branch is
recorded as never taken (delta).

## Music-read yield: 0x9A MUSICREADWAIT

Retail holds — re-delivering the same instruction every tick with no advance — whenever a music read
is in flight: it compares an index-bounded entry of a song table against the music server's current
slot value, and when they match asks the server whether a read is running; only "no read running"
advances past the one-byte opcode. Corpus: 4696 uses / 184 zones — typically right after a MUSIC
opcode. kuluu's music state lives in the renderer behind the cue boundary, so the VM cannot observe
any of this. Implemented as an unconditional single-pass yield (hold exactly once, advance on the
next delivery): strictly closer to retail than advancing in-tick when reads are running — which is
the normal case at those sites — and at most one frame off when they are not. Named as authored
choice against the unobservable predicate (delta).

Read to the bottom 2026-10-29 late, naming every gate: index = signed byte global; song table word
addressed by it compared against the tracker word; on match the busy test is a pending-counter pair
(two dwords, request vs completion) that also sets a refresh-request byte as a side effect — and
every pass of the opcode, holding or advancing, raises the VM yield byte. The globals are loader-
internal client state with no kuluu counterpart, so the single-pass stand-in stands (delta row
re-scored wording only).

## Scheduler stages 0x51 / 0x72: display-colour and flash-colour drives

The stage-type dispatch of the scheduler task factory is a jump table indexed by type byte minus two
(below it, everything at or past its bound falls to one unknown-stage label). The two colour-drive
stage kinds each allocate the *same* 0x4c-byte task class — identical constructor call sequence,
identical vtable, differing only in the stage-type tag stored inside the instance — which confirms
the record from earlier today on a stronger footing than "measured payload": retail drives its
display-colour and flash-colour registers through one task type recording which register it owns.

Both stages carry their destination as three bytes of the payload dword (red, green, blue in file
order at consecutive bytes — matching every measured triple: full-white drives read FF,FF,FF and
black-out drives 00,00,00 with alpha byte 80) and their length as a signed word scaled by the
routine's speed-ratio float before conversion to the task's countdown, exactly like the existing
fade stage's timing. A third global byte is set when either allocates (a "screen staging active"
switch flipped once at build; its consumers unread here). Kuluu now *classifies* both raw types with
their measured payload through its `ScreenColor` type — no longer unknown-stage skips reported only
by the coverage warn. Rendering them as two additional composite registers stays open on the same
blocker named in today's scheduler record: retail's task tick (behind the shared vtable) and its
compositing order against the existing fade register are unread, and kuluu will not guess a
composite order that visibly differs when several drives overlap.

## Where FFXiMain gets its opcode table (the dispatch itself, 2026-10-29)

**No DAT defines opcode behaviour.** The answer to "is there a table of how to perform each opcode" is: yes, and it lives
inside `FFXiMain.dll` itself, as machine code — the client never loads instruction definitions from data. What the DATs
carry is only the script byte stream that this built-in mapping interprets.

Observed shape of the mechanism (addresses, mnemonics and the slot values are under Provenance):

- One interpreter loop per event VM. Each step reads the script's instruction pointer for that VM, fetches the opcode
  byte at it, and hands the opcode to a dispatcher.
- The dispatcher range-checks the opcode against the highest one this build implements. Anything above falls through to
  one shared path — the client's own "this opcode does not exist" route, which is what kuluu's unimplemented-opcode
  result corresponds to.
- In range, it indexes a **jump table whose slot number equals the opcode value**: stride four bytes, one slot per
  opcode from zero through that highest implemented opcode, i.e. **219 slots** in this build. Every slot points at a tiny
  two-instruction stub — call the handler, return — so each opcode's behaviour is an ordinary function of the same
  image: 218 distinct handlers across those 219 opcodes (the end-of-script opcode and two late opcodes share one).
- After the handler returns, the loop re-reads that VM's park flag before taking the next instruction. That is the whole
  mechanism behind parking: a handler that leaves the instruction pointer where it was makes the script replay the same
  instruction instead of crashing or skipping.

Consequences for kuluu: an opcode is either performed by one specific client function or it is not implemented anywhere,
so "our opcodes are BAD" is answerable per opcode — diff the arm against its counterpart function. `scripts/opcode-dispatch-table.py`
regenerates the full mapping from this build (slot values out of the unpacked image, handlers resolved through their
stubs) together with kuluu's own coverage; it refuses a candidate table unless four opcodes already pinned in these
records resolve to exactly their cited handlers. Measured over those 219 opcodes: **153 have an arm in kuluu, 1 has a
named constant and no arm, and 65 are unnamed in the crate** (the coverage reading is textual, so where one opcode
carries two constants across files its state column can under-read; the client-side half comes from the image). The
unnamed set:
`04 12 13 16 17 18 2F 30 33 3A 3D 3F 57 59 5F 60 61 64 65 6D 71 74 75 7A 7B 7C 83 84 85 86 91 93 95 96 97 9C 9D 9E A2
A4 A5 A8 AA AB AC AD AE AF B1 B4 B5 B6 B7 B9 BE BF C0 C2 C3 CA CB CC D3 D8 D9`.

The table plus each slot's resolved handler and kuluu's coverage are regenerated by
`python scripts/opcode-dispatch-table.py --out .agents/skills/retail-observe/references/data/opcode-dispatch-table.md` (that path holds the current 219-row dump; `--full-stub-column` adds each stub address): slot values come out of
`research/xi-on-anything/generated/FFXiMain.unpacked.dll`, the stub→handler reading and the opcode bound out of the
generated C, kuluu's constants/arms out of `ffxi-event/src/vm.rs` and `ffxi-event/src/vm/scene.rs`. The script accepts a
switch table only if four record-pinned opcodes (0x1F→`f_100b2ea0`, 0x63→`f_100b7550`, 0x6E→`f_100b7c40`,
0x99→`f_100b7ce0`) resolve to exactly those handlers through the slots, so it cannot silently pick an unrelated
switch table. Measured over `0x00..0xDA`: **kuluu has 153 armed opcodes, 1 named constant with no arm, and 65 opcodes for
which the crate has no name at all** (the coverage axis is text-based, so where one opcode carries two constants in
different files the state column can under-read an arm; the handler side comes from the image). The unnamed set:
`04 12 13 16 17 18 2F 30 33 3A 3D 3F 57 59 5F 60 61 64 65 6D 71 74 75 7A 7B 7C 83 84 85 86 91 93 95 96 97 9C 9D 9E A2
A4 A5 A8 AA AB AC AD AE AF B1 B4 B5 B6 B7 B9 BE BF C0 C2 C3 CA CB CC D3 D8 D9`.

## The parity ledger and the store-family pass (2026-10-29 night)

`scripts/opcode-parity-ledger.py` is now the per-opcode oracle: for every dispatch slot it reads what
the handler body does to the two words of event state that steer execution — the exec pointer (stepped
past by a constant, by a computed amount, or set outright = a branch) and the yield byte the
interpreter loop clears on entry and re-reads after every single instruction-dispatch until any handler
sets it non-zero — and joins that against kuluu's declared width/jump/yield metadata and arm presence.
Verdict buckets: MATCH (every machine-checkable field agrees with an armed opcode), MATCH-ADVANCE
(retail performs nothing beyond stepping past itself by exactly the width kuluu already skips, so for
those the skip IS the 1:1 response), NO-ARM (retail runs code; kuluu size-skips or refuses), UNAUDITED
(computed widths, paths a machine cannot rank, thread-level ops). Human-audited rows are overridden
inside the script so regeneration never loses them. Tally after this pass:
**123 MATCH / 2 MATCH-ADVANCE / 55 NO-ARM / 39 UNAUDITED**, over all 219 slots.

Six store-family opcodes armed from their handlers, plus two END twins:

- Two random stores: the draw is one step of the CRT linear-congruential pair and its top-bitfield
  output; kuluu keeps the same state transition per VM (seeded to the CRT default), and when the range
  variant's modulus wraps from an operand, retail skips the division **without drawing** — so kuluu
  must not consume the stream either, pinned by test.
- Sine/cosine stores in heading units (4096 to a full turn): angle at one work operand scaled by a
  captured float constant, multiplied by an amplitude operand, sine's product negated before rounding,
  cosine's not; both funnel through the image's round-to-nearest-even qword-store conversion.
- Vector-to-heading: atan2 of (negated one operand, the other), scaled by two captured float constants
  in that order — half a turn comes back as ±2048.
- Remainder store with divide's both-operands guard (either zero stores zero; the value stored is the
  truncated remainder).
- The two END twins: all three dispatch slots land on one handler which resets the thread slot and
  yields; under kuluu's one-VM-per-block model ending this VM is the identical response. Before this
  pass, events containing either twin were refused outright mid-cutscene.

Three deferred with named side effects so a skip can never silently drift:
the frame-factor add reads a clamp-to-one global time factor that has no kuluu counterpart yet; one
byte-flag opcode's client-global has exactly two other static uses in the UI half of the image; and
one clear-a-global opcode advances by one, so width-skip stays in sync — its side effect is recorded,
not modelled.

### Bit-array clear, the entity flags-word twins, and the heading read: 0x3D / 0x2F / 0x33 / 0x3A (2026-10-29 late night)

Four more NO-ARM slots armed from their handlers, moving the ledger to **127 MATCH / 2 MATCH-ADVANCE /
51 NO-ARM / 39 UNAUDITED**:

- **0x3D BITARRAY_CLEAR** is the exact inverse of the already-armed 0x3C SET: same seven-byte form,
  operand 3 carries the flat bit index (split slot/bit exactly as BITTEST splits), operand 5 bounds
  the array, and the write is a read-modify-write of the word addressed at operand 1 — retail loads
  that word first (`getworkofs(1, slot)` through the same helper pair) and stores it back with only
  the one bit cleared. A bound refusal changes nothing and still advances.
- **0x2F / 0x33** rewrite *bit 19* and *bit 21* of the entity's **first** flags word (`[entry+288]`,
  not the `[entry+300]` render word the 0x92 family toggles). Both xor-latch: the inline byte is
  shifted onto its bit and the xor-and-mask isolates exactly `byte & 1`, so the stored bit equals the
  operand's low bit — value=false is a real write that clears, not a no-op. 0x2F takes its actor from
  a 0x92-style four-byte lookup at +2 (width six); 0x33 always acts on the block's own event entity
  (width two, the inline byte alone). Neither names what its bit does on screen; both ride the same
  `ActorRenderFlag` cue in the first-word bit space (`EventCue::ENTITY_FLAG_BASE`, following the
  precedent set by `RENDER_FLAG_VISUAL_BYTE`) with no renderer owner yet.
- **0x3A** is a *read*: it stores the named actor's heading back into the script at work operand +5,
  seven bytes wide with the actor lookup dword one byte earlier (+1) than any flag-family twin. The
  heading comes from the entity's radians float `[entry+24]`, or — when bit 7 of that same first
  flags word is set — from an interpolated chain `[[entry+212]+608]+340`; either way retail
  wraps it into [0, 2π) (`f_100dd9d0`: subtracting its 6.283185 dword while above, adding it back
  below zero), scales by the captured pair 4096.0 × 0.1591596 (=1/2π) and rounds nearest-even into
  the store. A **null entity-table slot stores zero**; an ineligible lookup advances past with no
  write at all — two distinct outcomes kuluu cannot separate (its resolver answers one question), so
  unresolvable actors share the zero law, stated as delta. In kuluu's integer heading space the whole
  float chain is exactly a positive modulo into [0, 4096).

### The entity turn/move speed switch: 0x59 (`OP_ENTITYSPEED`, nine cases)

Slot 0x59 — already named `OP_ENTITYSPEED` in the crate (doc name "entity turn/move speed") and
carried by the shared sub-byte width handler — is a nine-case staging-write family keyed on the
inline byte at +1; the handler read confirmed every case width against the table that was already
in `opcode_meta::sub_size` (4 for cases 0/2/7, 8 for 1/3/4/8, 7 for 5, 6 for 6). The cases are now
handled individually in a dedicated arm; only the widths had been modelled before. Cases:

- **0 / 2** store a work operand (@+2) into the *block's own entity*'s custom dword slots **+220**
  and **+224**; **1 / 3** are the same pair addressed through an actor lookup dword @+2 with the
  work operand at +6. No opcode anywhere in the dispatch reads those slots back; the only client-
  side readers found sit on objects carrying the mover state-block mirror family (`f_100ac503`,
  `f_100c4820` read `[x+220]` where siblings +212/+216 are the mover working quad, and one entity-
  init site defaults both slots to **100**). kuluu parses each store faithfully and applies nothing —
  named in Not established rather than dropped.
- **4** looks an actor up (@+2), takes a work operand @+6, converts it to float and stores
  `value × 0.1f` (scaling constant pinned in Provenance) into field **+360** of the actor's interpolated render
  object — the same `[[entry+212]+608]` chain whose +340 float is 0x3A's heading source. Parsed,
  not applied (same reason).
- **5** is a third first-flags-word twin: bit 21 (like 0x33) := low bit of an inline byte — but at
  operand offset **+6**, with the actor lookup dword @+2. Publishes the `ActorRenderFlag` cue in the
  `ENTITY_FLAG_BASE` space.
- **6** requires `[entry+452]` and the visual half `[entry+160]`, calls the visual object's
  dispatch entry **+672** (receiver = visual, arg = `[entry+452]`), and **parks on itself while
  that answers 1**; every
  other exit (including all absent-object branches) advances six. kuluu has no visual object, so it
  takes the advance path unconditionally — retail's own absent-actor behaviour, named against the
  unread vtable internals.
- **7 / 8** hand the work value to `f_1008a790`, which reaches the visual object's dispatch entry **+556**
  (this
  = `[entry+160]`, arg = value) — again visual internals; kuluu advances with the case width.
- **Selector past eight** re-dispatches without advancing ExecPointer at all: a retail spin, so no
  authored corpus byte can hold one and kuluu refuses (same posture as 0x72's unknown mode).

### The two distance stores: 0x64 / 0x65

Both are eleven bytes wide, store through a work operand at +1, and round nearest-even through the
image's integer conversion (`f_10311c6c`):

- **0x64** (`f_100b75c0`) is pure script math: fetch G(7), G(3) → dx = first minus second; G(9),
  G(5) → dz likewise (in that order); store `round(sqrt(dx²+dz²))`. Integer squares in x87
  registers, so no precision question beyond the rounding.
- **0x65** (`f_100b7860`) is a two-actor distance: lookups at +3 and +7 (both gated on eligibility
  and a non-null table entry), each actor's three-float position from its raw triple unless bit 7 of
  the first flags word promotes to the interpolated render chain's triple. The squared-sum
  square-root is scaled by **1000.0** (constant pinned in Provenance) before rounding. Unlike nearly every
  other actor op, **it always stores**: any failed lookup or absent entry pushes a zero into the
  same store — so kuluu's `named_actor_position`-based arm (one tracked position per actor;
  thousandths space already equals retail's raw×1000 scale) stores exactly the same law, and the two
  no-data outcomes collapse to that zero as elsewhere.

### The thread-slot timer behind the waits (0x1C / 0x6F / 0xB2 read)

The wait family does not keep its countdown where kuluu might expect — it lives in a **per-thread
float slot of the event control block**: `[event + thread_slot*32][+60]`, initialized to −1.0 by
the END handler and shared by exactly three dispatch sites: `OP_WAIT` (whose width-3 form loads its
duration operand into a *negative* slot once, then counts down), **0x6F SLEEP** (identical countdown
but arming the default **16.0** when the slot reads negative — no operand at all), and 0xB2
(delivery family). Each pass subtracts the clamped frame factor from one file-scope global (the
same ≤ 1.0 helper behind deferred 0x57's add) into the slot, sets the yield byte unconditionally,
and advances past itself only when the stored value drops **strictly below zero** — kuluu's
timed-wait clock reproduces all of that for WAIT (pinned by `a_zero_length_wait_still_costs_a_tick`
et al.), SLEEP with its 16-unit default (`SLEEP_WAIT_UNITS`), and B2; what kuluu does *not* model is
the sharing: retail scripts could pre-seed one wait's slot via another op of the family. No authored
corpus pairing that exploits this was sought — named, unchased.

### Turn-wait: 0x70 (`OP_TURNWAIT`) law read, kuluu gap stated

Retail parks on itself (yield byte, no advance) while bit 1 of the block entity's **second flags
word** `[entry+300]` is set — the turning/animating bit — and only on the passing edge does it
poke `f_1008af20`, which calls the visual object's dispatch entry +668 twice with FourCCs **"@tl0"** and
**"@tr0"** (two animation channels) when the actor also carries flags-word bit 9 of `[entry+288]`.
Absent entries advance one immediately. Kuluu's current arm advances one unconditionally — turn
flights in cutscenes therefore finish silently instead of gating the next instruction. Faithful
fix needs a VM-visible turning state (the animation-yield machinery is close); recorded as the
named gap rather than approximated here.

Ledger after this whole session: **132 MATCH / 2 MATCH-ADVANCE / 37 NO-ARM / 46 UNAUDITED /
2 WIDTH-ONLY** — the NO-ARM drop from 51 is partly
visibility (the coverage scanner had been blind to constants defined in `opcode_meta.rs`, so the
shared sub-byte families counted as unarmed; they are width-handled but mostly effect-unread, which
is what UNAUDITED and WIDTH-ONLY now say honestly) and partly this session's new arms.

### The entity-flags latch family and the third-word writers: 0x61 / 0x60 / 0x74 / 0x7C (2026-10-29 late)

Four handlers in one address band, all writing the same three entity flags words of the block's
own entity-table entry (the VM's own index is read as a word at object offset 2 and used to walk the scene
entry table, empty-slot skip on every path — addressing in Provenance):

- **0x61** (`f_100b74e0`, width 2): `[entry+296] bit0 := byte@+1 & 1` written as a mask-OR
  read-modify-write — a plain store of the bit, categorically NOT the xor-latch form below.
- **0x60** (`f_100b73f0`): the selector op. Byte @+1 picks both width and law: sel ≤ 1 xor-latches
  `[entry+292] bit30` to the selector itself (retail's sign-extend/shift/mask latch arithmetic
  isolates exactly that bit — instruction stream in Provenance), advance **4**; sel == 2 resolves an actor whose lookup
  token is the selector value pushed as-is (`push ecx` with ecx=2 — i.e. the event-entity token,
  not a dword read from the stream) and hands it to the singleton named by a global pointer
  (Provenance) through its dispatch entry **+24** (two zeroes fed in first), advance **6**; any other selector takes retail's own default arm
  — `add exec, cx` with ecx=2 — exactly **two**, not a spin. The table row claiming size 6 is the
  widest-case fiction; widths now sit in `sub_size` as 4/6/else-2.
- **0x74** (`f_100b7490`, width 2): third member of the word-124 latch family — after 0x5F's
  bit 29 (cases 0/1, read earlier this band) and 0x60's bit 30 comes **bit 31**, xor-latched to
  byte@+1 & 1 (`shl 0x1f`, mask `0x7fffffff`).
- **0x7C** (`f_100b8a80`, width 6): `[entry+296] bit17` set-or-cleared from whether byte@+1 is
  non-**zero** — `or 0x20000` vs `and ~0x20000`, so unlike the low-bit twins any value ≥ 1 sets it.
  Its handler first demands the standard eligibility word (`f_100afac0` token 2 → `f_100afb00`
  == 1) and a non-null entry; both gates fold into kuluu's publish-anyway posture as elsewhere.
- **0x7B** (`f_100b8a00`, width 5, read to the last instruction this pass but not armed): resolves one
  actor operand through the fetch/resolver pair (token at +1), demands both the resolver's eligibility
  answer and a non-null table entry with bit 9 of its eligibility word set; then **clears bits 8–9** of the
  fourth flags word [entry+300] (the earlier "top six bits" wording was wrong — it is two bits, mask and
  store quoted under Provenance) and stores the all-bits sixteen-bit value at **[entry+328]**, whose only
  known reader question stays open in Not established #7. Every gate-fail skips both writes and still
  advances the width. Stays NO-ARM: neither effect has a kuluu channel.

Kuluu arms: dedicated `OP_ENTITY_WORD128_SET` / `OP_FLAGWORD_SELECTOR` /
`OP_ENTITY_WORD124_BIT31_LATCH` / `OP_ENTITY_WORD128_SET_BIT17` in the dispatch, plus a
`(OP_SUBSCHED, 0|1)` family arm for 0x5F's bit-29 latch cases (sel=0 stores zero there —
`shl 0x1d` of 0; sel=1 sets), all cueing in the new sibling bit spaces
`EventCue::ENTITY_WORD_124_BASE` (0x40) and `ENTITY_WORD_128_BASE` (0x60), following the
`ENTITY_FLAG_BASE` precedent — hosts route the words, the VM keeps no addresses.
The 0x60 case-2 singleton handoff is parsed-not-applied (slot +24 internals unread); 0x5F's
cases ≥2 keep advancing their recorded widths pending the helper read below.

The scheduler-helper family these sit next to was sketched but not finished: `f_100b5510` /
`f_100b5210` (used by 0x5F cases ≥2 and by 0x66's stub directly) resolve two operand tokens
(`push N` = the dword at stream offset N → entity-table slot), demand both entries' id-pair fields
`[+116]/[+120]` match the fetched pair, both `[entry+288]` bit 9 and bit 7 set, then load a
motion (threshold buckets on `f_100af4b0` values ≥ 0xc00 / 0x800 / 0x400 / 0x200 adding literal
offsets to a message base) through `[entry+160]` handles and set the yield byte from the load;
false makes every caller **rewind exec to the opcode** — retry-per-tick, kuluu's `Waiting` idiom.
The bucket constants' authored meaning (which motion packages they name) is the remaining blocker
(Not established #7). 0x66 itself is already armed in kuluu as `OP_LOADEXTSCHEDULER2`; its ledger
row stays UNAUDITED on that helper path until this read is finished.
*(That sketch's rewind law — "false makes every caller rewind" and a null slot returning false —
is retracted below by the completed read; the sketch stood only as sketched.)*

Ledger after this batch: **135 MATCH / 2 MATCH-ADVANCE / 33 NO-ARM / 47 UNAUDITED / 2 WIDTH-ONLY**
(0x61, 0x74, 0x7C machine-MATCHed on new arms; 0x6D audited as a literal `add +7` empty stub — no
behaviour to miss; 0x60 audit row overrides the computed-width UNAUDITED bucket with the selector
table above).

### Body-read audits, one band: 0x21 / 0x23 / 0x24 / 0x25 / 0x26 / 0x2E / 0x30 / 0x3E / 0x42 / 0x4C / 0x4D / 0x58

Twelve UNAUDITED rows closed by reading each handler to the last instruction (no code change needed —
every one lands on a law kuluu already implements or deliberately declines):

- **Yield trio**: `f_100afc70` (0x26) sets only the yield byte, never exec → dispatched forever,
  which is exactly what kuluu's parked `OP_YIELD_FOREVER` is; `f_100b0380` (0x58) incs exec then
  yields; both machine-verified after the read. `f_100b2dd0` (0x23 MESWAIT) is a four-state machine
  on a named global byte (address in Provenance): state 0 advances and clears the thread's speaker slot ([thread+356]
  through to `[entry+334]:=0`, with an optional `@`-channel animation release through the entry
  handle), state 1 yields in place, state 2 writes three end-bytes and yields without advancing.
  Kuluu's message-await hold/advance is the same stream law; the global half stays unmodelled.
- **End-request** `f_100b34e0` (0x21 EXECEND): sets a client request byte, mirrors another global
  into it when clear, then yields with no advance — park-on-end. Kuluu ends the VM explicitly,
  audited as END's posture.
- **Global-byte pokes** `f_100b55d0` / `f_100b5600` (0x2E/0x42 cancel arm/disarm) and
  `f_100b5620` (0x30): each touches only its named global (three addresses pinned in Provenance)
  and advances one byte. That last write answers this record's long-standing
  "0x30 reset-flag" deferral: it is a plain reset, no operands read — the width skip *is*
  kuluu's faithful behaviour, so 0x30 leaves the NO-ARM bucket with an audit reason and gains no
  arm (an empty arm would be dead code).
- **BITTEST** `f_100b0250` (0x3E): flat index @+3 → slot via `sar 5`, bit via `and 0x1f`; set →
  add 7, clear → jump to u16 @+5 — the identical split kuluu's `BIT_TEST_WORD_SHIFT` /
  `BIT_TEST_BIT_MASK` constants encode.
- **Door pair** `f_100b6950` / `f_100b6a00` (0x4C/0x4D): on the block entry, when low byte of
  `[entry+288]` bit2 is clear, store status **8**/**9** at `[entry+372]`, advance one always —
  confirming kuluu's `STATUS_EVENT_DOOR_OPEN=8`/`CLOSE=9` values and its unmodelled-flag-gate
  posture from the earlier door-pair section.
- **Query pair**: 0x24 is a one-jump thunk (`jmp f_100b25f0`, body not read — level of audit stated);
  `f_100b2940` (0x25 QUERYWAIT) polls a result word: zero yields without advancing (retry per tick
  = kuluu's ack park), non-zero advances and posts the answer to globals, with an **0xFF** branch
  storing 0xFE plus three end-bytes that kuluu does not reproduce (globals; named here).

- Two more from the same band: `f_100b87b0` (0x78 restore-clock) conditionally on a paused-state
  byte calls two clock-singleton refreshes and clears one visibility slot, always advancing —
  kuluu's cue posture again; `f_100b93c0` (0x8A close-map) is one map-window teardown helper call
  then `inc exec`, no operands read. Kuluu pins for both arms already exist (`OP_RESTORE_CLOCK`,
  `OP_CLOSE_MAP`).

Ledger after this audit band: see regenerated census below (`scripts/opcode-parity-ledger.py`);
the remaining UNAUDITED rows are the ones needing real semantic work (scheduler helpers behind
0x5F, map/menu families B4–B7, computed-width CC/D8 and the deferred-globals list), each still a
named row in the queue.

### Scheduler-helper family, read complete — with retractions (2026-10-29)

The sketch above mis-read three branch directions and got the case layouts wrong in detail; both
discrepancies matter to parity, so they are settled here from the dispatcher, its jump table, and
each helper read instruction-by-instruction.

**Retractions.** (1) "Failed actor resolution makes every caller rewind exec" — **wrong**. In all
three helpers (`f_100b5510`, `f_100b5210`, `f_100b4e10`) an invalid token fetch, a null entity-table
slot, an id-pair mismatch (the entry's two scheduler-id fields against the fetched pair) or a
refusal on either of the entity flag word's ready bits branches to that helper's **advance tail**:
exec steps past the full case width, and the caller-supplied operand byte is stored into the
scheduler yield byte. The reason the sketch inverted it: on a quick read the resolution gates look
like they land on a shared failure block; dumping each branch target exactly shows them landing on
the width-advance tail, while the *only* false-returning paths are the late handle check and the
busy predicate (below). (2) "Null entity returns false" — wrong for the same reason: in both
`f_100b5510` and `f_100b4e10` the null-entry branch goes straight to the advance tail; a null slot
skips, it does not park.

**Dispatch (`f_100b72c0`; its jump table was read from the image data — Provenance).** sel 0/1 →
shared latch case (already armed); sel 2 → `f_100b5510`; sel 3-6 → `f_100b5210` with three dword
args; sel 7 → `f_100b4e10` with two. Every helper case first sets exec one past the opcode —
retail's operand fetches (`f_100afac0`, and its word-fetch twin) read at *that* base, so absolute
stream offsets below are from the opcode start. When a helper returns false the dispatcher restores
exec to its entry value (kuluu's parked `Waiting`, retried next pass); when true it leaves the
helper's own advance.

**Layouts and widths per case:**

| sel | handler | inline operands (abs stream offsets) | width |
|-----|---------|--------------------------------------|-------|
| 2 | `f_100b5510` | actor dword @+2 (single fetch) | 6 |
| 3 | `f_100b5210` (A=0, C=0) | id word @+2, actors @+4 and @+8, action-key dword @+12 | 16 |
| 4 | `f_100b5210` (A=1, C=0) | as case 3 | 16 |
| 5 | `f_100b5210` (A=0, C=1) | as case 3 plus a trailing operand word @+16 — carried exactly by the cases whose C byte ≠ 0, which is what the width bump buys | 18 |
| 6 | `f_100b5210` (A=1, C=1) | as case 5 | 18 |
| 7 | `f_100b4e10` | actors @+2 and @+6; a numeric predicate argument dword @+10 fed to the motion-object call — not an actor | 14 |

The widths land exactly on `opcode_meta::sub_size`'s existing rows (6/16/16/18/18/14) — the meta
table was right; only this record's earlier sketch had offsets wrong.

**Park conditions, exhaustively.** Cases 2-6 park only at their late motion-handle gate: once both
entity slots exist and pass the id-pair and ready-bit demands, `f_100d12d0` on the entry's visual-
handle field returning false sets yield byte 1 and rewinds exec. On true — and for cases 3-6 gated
additionally by a per-slot scheduler pending-load byte (slot index = argument A, 0 or 1; zeroed
again after the load) — they *load*: `f_100d2110` with the bucketed file id, or `f_100d2220` if the
id transform took its table branch, and for cases 3-6, when the action-key dword is neither zero
nor 'xxxx', a start call on the handle's virtual dispatch (slot pair in Provenance) with the
trailing word. Case 7 (`f_100b4e10`) parks while its busy predicate holds: after both actors
resolve and each reports ready, it calls one of two motion-object query virtuals on actor 1's
handle — which one depends on whether the case's rightmost operand byte is non-zero (case 7 passes
1) — passing actor 2's handle and the @+10 dword; a busy answer sets yield byte 1 and rewinds. A
leftover non-zero yield byte parks it too. On free: advance tail, yield ← caller's operand byte
(always 0 for the dispatched cases).

**Scheduler-id transform.** `f_100af4b0(N)` fetches a stream **word** at exec+N through the word-fetch
twin and runs it through `f_100af4c0`'s ranges: small values are replaced by dwords indexed from the
scheduler state block, mid-range values are registered into a global table and come back as **zero**,
and the largest range folds one band size down before continuing. The bucket chain compares its
thresholds against this transformed value and adds each band's anchor to it (constants in
Provenance). So kuluu's `scheduler_file_id` anchors/arithmetic stay pinned, but its former raw-word
inputs (+B/+D) were wrong; the arm now parses no id words at all (nothing consumes them), and what
feeds the chain precisely stays Not established (#7 rewritten below).

**Kuluu after this read:** the `OP_SUBSCHED if selector ≥ 2` arm advances cases 2-6 by their
sub-size unconditionally (resolution-neutral, matching the advance tails; only a kuluu-modelled
load-pending state could park there and none exists), and parks case 7 through exactly one gate —
both actors resolving in the tracked entity map **and** `action_running_any` on either. Tests:
`subsched_helper_cases_advance_whether_their_actors_resolve_or_not`,
`subsched_case_seven_parks_while_an_actor_is_busy`, plus the untouched latch and bucket tests.
The previous session's two failing tests encoded this record's retracted rewind law (and looked up
one actor through the scene fallback while retail treats a high-byte operand as a literal server
id); they are rewritten to the corrected law, not patched around.

### The small-ops read pass: conditionals, the send-tag pair, door/flag/staging gates (2026-10-29)

Both sides of fourteen open rows were re-read to the last instruction this pass; kuluu's arms agree,
so they close as ledger audits rather than code changes. Per-op law:

- **0x02 `OP_IF`** (`f_100afd10`). Eight words wide. The low nibble of the byte at word offset +5
  selects a comparison over an eleven-entry table; kinds past the table take the branch. Operands
  are the work slots named by words +1 and +3 (each fetch runs before dispatch but its value comes
  from the slot). Kinds: 0 unequal, 1 equal, 2 ≤, 3 ≥, 4 <, 5 > (signed); 6 and 9 jump when the
  bitwise AND of the two values is zero; 8 when their OR is zero; 10 jumps when every bit of one
  value is contained in the complement of the other — a subset test. Taken → absolute jump to the
  address at word +6 (via `f_100afa90`); not taken → advance eight past the instruction. Kuluu's
  `op_if` answers kind-for-kind, out-of-range included.
- **0x43 / 0x47 send-tag pair** (`f_100b63a0`, `f_100b62b0`). A mode byte at word +1: the zero case
  performs the send (the tag itself; or for the position tag, resolve the work operands at +8/+6/+4/+2,
  scale to floats and hand them to the motion/position setter), sets an in-flight latch global when it
  succeeds — two latches for the position pair — and takes its width (+2 and +10); a failed send holds
  without advancing. The one case polls: while the latch is set hold, once clear advance two (and
  yield). Any other case byte spins holding forever; authored data cannot carry one (kuluu advances,
  as documented at both arms). Kuluu models the latch as its pending server-ack round-trip — the same
  pairing law already audited for the A7 sibling.
- **0x44 `OP_ENTITY_VALID`** (`f_100b6400`). Resolve the entity id in work operand +1 against the pool:
  present → advance five words, past the instruction; absent → jump to the address at word +3. An
  if-not-without-else of exactly one body. Kuluu is exact.
- **0x5B / 0x66 `OP_LOADEXTSCHEDULER` / `...2`** (`f_100b51f0`, `f_100b5200`). Both are thin wrappers
  over scheduler helper `f_100b5210`, whose full body the Scheduler-helper section above records; they
  differ only in the mode flag that selects plain-motion versus TPC-package form. The accept gate is
  the entity-type set (plain {1,2,7,8}, packages {0,1,6}) and kuluu arms exactly that, rejecting a
  null or all-zero key without cueing; refusals land on the helper's width-advance tail, which kuluu's
  unconditional fifteen-word advance reproduces.
- **0x8E / 0x8F second door pair** (`f_100b6980`, `f_100b6a30`). Re-read confirms this record's own
  section: status values 45 and 46 into the block entity's status slot (+372) when the entry resolves
  and its gate bit ([entry+288] bit 2) is clear; the instruction always advances one — via an
  increment, which is why the width classifier saw no add. Kuluu's cue carries exactly those values.
- **0x90 `OP_EVENT_HIDE_ALWAYS`** (`f_100b3540`). When the block entity resolves: invoke its hide
  method (`f_1008e4b0`, argument 1) and set a bit in the third flags word ([entry+292] high byte,
  bit 4); always advance one. Kuluu cues the hide half and advances; the unnamed-bit half keeps the
  posture the record states for it.
- **0x9B `OP_ANIM_YIELD`** (`f_100b7520`). Hold while the resolved block entity's animation-running
  byte ([entry+444]) is non-zero; otherwise advance one. Kuluu parks on precisely that shape against
  its own per-entity report.
- **0x98 `OP_ZONE_READ_YIELD`** (`f_100b8630`). Poll the async-loader singleton (`f_1018ba60`) — busy:
  hold, not busy: advance one. Zone reads never run asynchronously below kuluu's cue boundary, so its
  unconditional advance is retail's not-busy path; same posture as the map-reload row.
- **0x82 `OP_RANGE_RECT`** (`f_100b9040`). Resolve region id at work operand +1, take the block
  entity's model bounds (four floats, [model+52 through +64]) and ask the singleton region manager
  (`f_1017ad70` then `f_1017ad80`): on a hit advance seven past the instruction; every miss path —
  no entity, no model, no overlap — jumps to the address at word +5. Kuluu is exact.
- **0xC9 `OP_ENABLE_TIMER`** (`f_100b87e0`). Only while the clock-freeze global byte is set: resume
  the in-game clock (`f_1018c4d0`) and clear the countdown byte it parks on; always advance one.
  Kuluu's unconditional resume-cue lands the same state — the stop/restore pair beside it is audited
  as matching, this row closes with them.
- **0xA6 `OP_A6_SUBMAP`** (`f_100ba9c0`). Case zero allocates client packet id 235 and sets the
  request-pending byte, taking two words; an allocation failure holds. Case one holds while pending,
  else advances two (and yields). Case two stores the answered value global into the work operand its
  +2 names and takes four words. Kuluu's SubMapNum pending-ack arm reproduces the pairing law and the
  widths.

Read-progress, still open: **0x7F `OP_QUERYWAIT2`** (`f_100b2aa0`) — retail polls the UI query
singleton; while unanswered it holds without advancing, on an answer word it advances one and writes
the result into variable slot zero under a sentinel law (an answer of 255 stores as 255, anything
else stores answer−1 — menu ids are authored one-based), refreshing the widget either way. Kuluu's
arm holds through its choice/message model and remaps a cancelled selection to 255; comparing that
cancel-remap against retail's off-by-one write is what remains on this row. Re-reads of TURNWAIT,
MUSICREADWAIT and the map sub-op family confirm their recorded laws unchanged; all three keep their
existing delta rows and queue status.

### The window and menu families read complete: 0xB4 `OP_WINDOW` / 0x71 `OP_MENU` (2026-10-29)

Both handlers were read to the last instruction with their dispatch tables dumped from the image
byte-for-byte — they are UI-driving families, and kuluu's encoded case-width table answers
dispatch **exactly**, case for case; what separates these rows from a close is that several cases
drive widgets this VM does not model. Law per case:

**0xB4 (`f_100b0a00`, 22 cases, byte at word +1).** Two work-string ops are fully implemented in
kuluu and match rule-for-rule: case 0 copies the inline sixteen bytes at +4 into the work string
the +2 operand names (advance twenty); case 1 copies pending-table slot `clamp(word+4 to [0,4)
else 0)` into that string (advance six). The remaining cases drive three widget realms —
- chat-window title and pending-string tables: case 4 mirrors the work string back into both
  pending tables; case 2 sets up the nameplate label object from work-string slot two under a
  range gate on word +4, creating the named widget if absent (all advance six);
- query-window mode byte: case 6 stores its sub-byte value into the global mode byte and re-posts
  it to the query singleton — the same singleton QUERYWAIT2 answers from; cases 5, 3 and 8 poke or
  dismiss named widgets (widths three/two);
- two value widgets with open/get pairs: cases 7/9 and 0xA/0xC set a work value into widget object
  slots and read their float back into the work operand named at +2 — **with an always-store law:
  when the widget does not exist retail stores zero** (advance four); cases 0xD/0xE flip one global
  dword; cases 0xF-0x12 call two singleton pairs with fetched operands (six);
- a box-shaped marker: case 0x14 fetches five work operands (four scaled by the milli float),
  hands them to the map-singleton method, advance twelve — and its poll twin case 0x15 spins while
  that singleton's payload slot is non-null, else advances two. Kuluu deliberately does not treat
  0xB4/0x15 as an input-wait: this VM never creates the object, so retail's own absent branch —
  unconditional advance — is exactly what kuluu does.

**0x71 (`f_100b7e10`, byte-map over sub values to a 21-entry table).** Unknown subs land on the
no-advance spin; kuluu stops the VM there instead, which halts progress identically. The family is
the input/menu lifecycle: case 0 arms both latches and opens the named input widget (width two);
case 1 refuses while its latch is set, else allocates client packet **96**, fills it from the
input-buffer global (with a four-dword identity prefix), sends it and advances two — kuluu's tested
posture for this pair is to stop rather than fabricate credentials. Case 2 spins while the menu
open-byte holds, else advances two; case 3 reads the buffer through the string-length helper into
work +2 (width four). Cases 0x10/0x12 open numeric/string widgets with counts derived from work
operands (`inc`/+3 variants); 0x11 and 0x13 share one poll body — latch set spins, clear stores
the buffer's length into work +2 and takes four; 0x31 polls the string-input object itself. Case
0x20/0x21 hand off to the map-window helpers recorded in this record's map sub-op section (the
advance happens inside those helpers — kuluu's case widths sixteen/two describe the net).
Sub 0x40 registers a choice callback with fetch(+2) through the prompt singleton; sub **0x41** is
its poll: while the answered helper says false it spins, and on true it stores three result
globals into work +2/+4/+6 taking width eight — exactly kuluu's `{Menu, 0x41}` input-wait row of
width 8. Subs 0x50/0x51 set text into the name-object singleton (plus four) and release it (minus
two); 0x52 restyles it (plus four); **0x53** polls its finish state — null object advances two,
same as kuluu not gating 0x53 through input-wait; 0x54 computes three scaled-offset shorts from
the name-object geometry globals and hands them to the widget (width ten), and 0x55 resolves it
into work +2 with fetch(+2) (plus four). Every one of those widths equals kuluu's `sub_size` table
row-for-row — verified twice, the first pass by hand-mapping table entries wrong in this session's
own notes before the mechanical re-dump confirmed them; no SIZE-DIFFER anywhere in the trio.

**B6 (`f_100b1150`, guard ≤ 0x17)** completes the trio: every case reads or writes the **block
entity's look record** — target id byte (+239, with flags-word bit 0 set alongside), apply-flag
bits (+244/+245 and the shared tail’s latch of second-flags-word bit 21), slot words +252/+270
under compare-store-if-changed, whole look structs (target triple pairs through +266) at widths
twenty / fourteen / sixteen for cases 0x0B/0x0D/0x0E — and **sub 0x16, a wider whole-look variant
at width twenty-two that the md docs never encoded**. Case 0x0F stores fetch(+2)×0.1 as a float
model scale at +516; case 0x10 waits on the current entity's loading flag before taking two (the
documented loaded-path choice kuluu already makes); 0x11 hands fetch(+2) to the actor method
`f_1008a750`; subs 0x12/0x13 clear-or-set bit four of +296 exactly on the sub value; 0x14/0x15
resolve a named actor into +296 (width six); and **sub 0x17 re-runs the case-0xF scale store
against the current entity, width four**. The two widths kuluu had never encoded (0x16→twenty-
two, 0x17→four — it stopped on both instead of advancing) are now in its table from the handlers’
own exec-adds. Remaining unread queue: 9D STRINGOPS, CC ITEMINFO, 8C RECIPE, B3 RANKING,
AB renderflag, AC statusset, and helper `f_100b4590` behind 0x73/C4.

### The magic pair and their shared helper: 0x73 / 0xC4 via `f_100b4590` (2026-10-29 late)

The queue's last entry, and it closes differently. Both handlers are thin: 0x73 (`f_100b4540`) is a
two-instruction wrapper calling the helper with selector sixteen; 0xC4 (`f_100b4550`) dispatches
its sub byte — zero/one/two select selectors seventeen/eighteen/nineteen, any other sub falls
straight out of the handler **without advancing** (kuluu encodes no width there and halts: same
no-progress outcome). The helper (`f_100b4590`) is a gated visual-method dispatcher:
- resolves two actor lookups back to back through the eligibility pair; either failing jumps to
  the shared advance tail;
- both resolved entities' scheduler slot must be nonzero and **equal between the two** — another
  mismatch tail;
- bit 9 of each entity's second flags word must be set on both, else tail;
- only then a fetched selector byte (stream +11) indexes an eighteen-entry table: every case calls
  one slot of the second actor's visual object vtable with three operands — first actor id, a
  fetched value, and the four-character literal naming the scheduler entry (“main”); one variant
  passes two zero parameters instead;
- **every exit advances exactly eleven stream bytes** — success and each gate-fail tail share one
  add. The instruction's footprint is fixed at eleven whether or not anything ran.

That shared-tail law makes kuluu's current posture — no arm, generic advance by the declared size
eleven — faithful on every control path: when retail's gates fail nothing happens and the stream
steps eleven; when they pass it steps eleven after running a visual method. Widths verified equal;
the un-run half is the eighteen visual scheduler-entry handoffs themselves (which entry each
selector names, and what “main” resolves to inside the visual object, are unread below the vtable).
The corpus sweep refuses neither opcode; 0xC4's authored subs never exceed two in any zone.

### The string-variable family read complete: 0x9D `OP_STRINGOPS` (2026-10-29 late)

Last member of the queue, and the biggest body in the pass. Dispatch is **paired**: sub byte above
0x10 spins on a bare-return tail; the sixteen-entry table pairs each narrow case with a wide one —
cases 0x0A–0x10 land on the *same bodies* as cases 0x00–0x06 (wide form only), and inside those
bodies the word fetched at +8 selects the width itself: zero → the narrow advance (eight or six),
nonzero → ten. Kuluu's `sub_size` row encodes exactly these amounts, per sub, taking the narrow
form; it equals the handler case-for-case in both forms.

What the family works on is a **string-variable registry**, not just work slots: a dword table of
string handles with an index parallel — indices below 64 select registry entries (the store case
writes handle words straight into the table at index×4; a null entry routes to one named empty
literal block, a full one through its value twin), and an index **at or above 64 selects external
string data**: the fetch path re-bases reads of that instruction's own operand words off a literal
block pointer, which is why the wide forms carry one more stream word — kuluu executes these
through its fetched-operand getters, so the redirection composes without special-casing. Stores
into string slots go through the work-string setter twin with a sixteen-byte source at stride
two (slot index×2), and one compare case walks four candidate strings per side before it decides.
The two compare cases are also this family's **label-jump** mechanism: on the mismatch outcome
they fetch a jump-target word from +7 (the eight-candidate variant, plus the plain-equality
twins) or the twenty-first-operand twin and store it into exec directly — authored failure
arms branch to labels; kuluu currently falls through instead of branching, its one effect gap here.

Corpus posture: the VM sweep over all 299 zone event DATs refuses **no** 0x9D case (refused set:
71/31/5A/8C/B3/CC only) — every executed string op ran within kuluu's encoded widths. A static
linear-walk sub-byte census of this family is not trustworthy (the walks desync over inline-string
islands; several phantom “subs” above 0x10, which dispatch cannot even reach, show up), so the
effect laws above are from the handler body, and execution coverage rests on the sweep.

### The five body reads this pass and the QUERYWAIT2 close: 0xAB / 0xAC / 0xCC / 0x8C / 0xB3 (2026-10-29 late)

The queue left open by the window/menu section is now read in full — all five handler bodies,
to their last instruction, with every jump table resolved entry-by-entry. The headline: **no
SIZE-DIFFER anywhere** — kuluu's encoded case-width table answered every one of the five from
the handlers' own advance sites before this pass; what these reads add are the per-case effect
laws and the out-of-range behaviour, which separate a close from a named decision. A corpus
census (linear thread walks over the current install's 299 zone event DATs, 186,815 threads —
executed-along-walk counts, jump targets not followed) sizes the families by real authored use:
0xCC 3588 sites / 101 zones, 0xAB 2962 / 158, 0xAC 653 / 101, 0x7F 86 / 50, **0x8C 40 / 10
zones, 0xB3 28 / 6 zones** — the last two are rare enough that no shipped cutscene's main line
depends on them.

**0xAB render-flag (`f_100bac90`).** Guard: sub byte at +1 above 0x1C jumps to a bare-return tail
— **out-of-range subs advance nothing and spin**, like AC's, exactly what kuluu's no-width-encoded
halt reproduces. Widths are two words everywhere except six cases that read actor at +2 and a value word fetched at +4 for
the flag pair-hands (advance four), and sub 0x1B which fetches two operands (advance six).
Effects: bit set/clear pairs on the entity's second flags dword +288 (bits 0,2 / 1,3 / 6,7) and
third +296 (bit 1); writes 8/9 into **status word +372** — the same slot today's door-pair opcodes
write; a guard that consults status-word bits plus the animation-busy byte at +444 and halts the
case by writing the VM pending byte (five bytes into the instruction); two flips of one client-global overlay byte; six "fetch a value, hand it to a
window-singleton method" pairs; sub 0x19 xor-folds bit 19 of +316 against "sub equals 0x19"
(always-set law); sub 0x1B fetches two operands and sets that same bit 19 from the second one;
and sub 7 stores its fetched value into a client dword slot no event opcode reads back. Kuluu's
`sub_size` row equals the table case-for-case; its arm runs width-only — none of these entity-bit
effects are applied, in the same published-cue-not-applied posture as today's flags-word rows.

**0xAC status-set (`f_100bb180`).** Guard above 4 jumps to an add-by-register tail whose register
is zeroed at handler entry — **out-of-range subs advance nothing and spin**, exactly what kuluu's
no-width-encoded halt reproduces. Cases:
0/1 read actor at +2, fetch one value, store it into entity **status word +368 / +372** (advance
four — case 1's +372 is the door-status slot again); case 2 latches bit 31 of third-flags-word
+312 from a compare against sub==2 through the eligibility pair, calls a helper on the actor and,
when that helper reports false **and** the latch was clear, writes the VM pending byte without
advancing (advance six); case 4 fetches a third operand and ORs its low two bits into +312
(advance eight). All four widths equal kuluu's row; arms run width-only.

**0xCC item-info (`f_100b9d90`).** Guard above 0x20 lands on the advance-4 default tail —
kuluu encodes exactly that as `Some(4)` for sub 0x20 (verified this pass to be how out-of-range
subs are reached: a byte-map over sub values into a small table whose default index is the +4
body). The interesting bodies build an **eight-word item record** in one client block and hand it
to two window openers: case 0 fetches three values into the record then opens an examine-style
window over a one-word descriptor (if its actor operand resolves to nothing, a self-item variant
of the same window instead); case 2 zeroes the record, stages **seven words from +2,+4,+6,+8,+A
plus two fetched values**, pulls a full item record through the work-string helper's twin, and
opens the detail window (advance fourteen — the width kuluu pins); case 0x10 fills one word of a
different widget gated on its existence (six). Sub **0x11 is a player wait**: while a window byte
(+109 in that widget block) holds, it stores the pending byte without advancing; when clear it
stores a signed widget word into work +2 and takes four — exactly the input-wait kuluu refuses.
Sub 0x20's body hands a fetched value (zero → a reset call, nonzero → a set-with-index call) to
an engine singleton (four). What those two window families show on screen is not established; the
record mechanics are, top to bottom.

**0x8C recipe-send (`f_100b98e0`) and 0xB3 ranking-board (`f_100bbaf0`): send-and-park pairs.**
Both build **client packet id 88**, fill a command word plus operand words from the case's stream
words, finalize-send through the same alloc/finalize pair today's A6 sub-map row uses, arm the
cross-thread pending byte at that send global block, and write the VM pending byte — retail parks
every recipe/ranking flow until the server's reply lands. 0x8C: cases 0/2/3/4/5 read four to
seven stream words (widths eight/twelve/ten/ten/fourteen) with distinct command words; case 1 is
the poll — pending byte set → **no advance, yield only** (re-runs every frame), clear → the width
two tail. Out-of-range cases and allocation failures share the no-advance spin. 0xB3: nine subs,
widths {two, four, fourteen, eighteen} as kuluu encodes — three send command variants, two
board-state writes through the ranking singleton with a fetch(+2) selector (cases 6/7), one
**"copy the whole board into nine consecutive work slots"** case (eighteen, kuluu's encoded width
to the word), one clear-the-board call gated on the singleton existing at all — and every path,
send or not, yields. Retail never advances these without parking; kuluu refuses both opcodes
outright (`unsupported_query_workflows_do_not_send_or_fabricate_results` pins the refusal for
cases 0..10), which given the census is a named decision: nothing shipped walks past either on
its main line, and fabricating sends with no reply path would park events worse than halting.

**0x7F QUERYWAIT2 — closed as a match.** Retail's poll reads its query singleton, spins (yield,
no advance) while the window still holds an unanswered prompt, and once answered advances one
with the answer stored through the **one-based-to-zero sentinel law**: keep 255 for cancel, else
decrement. Kuluu stores zero-based choices into work slot zero at selection time (`select_choice`)
and remaps its own 254 cancel marker to 255 as it advances past the arm — so the authored
sequences downstream (`if (work0 == n)` against one-based-authored ids) observe identical values;
the tests `op_7f_querywait2_yields_then_resumes` and
`op_7f_querywait2_does_not_cancel_on_a_cancelled_choice` pin both paths. Single-threaded here,
there is no window between the store and the next instruction for anything to interleave.

## Mount-status switch: 0x7E `OP_CHOCOBO` (2026-10-29 late)

Nine-case sub switch, guarded at eight; the guard-fail branch takes a garbage width register off the
stack — authored data cannot hold an out-of-range sub without running exec away (the sweep confirms
none). Widths come from the handler's first jump table: six for subs 0/1/2/4/5/8, sixteen for 3,
eighteen for 6, eight for 7 — equal to kuluu's `sub_size` row case-for-case. A triple gate (entity
eligibility pair on the word operand @+1; table slot non-null; visual-target bit in the flags dword)
fails to a shared tail that advances by the case width with nothing applied — exactly kuluu's
unresolvable-target posture. The second dispatch stores mount-status values into the door-status
dword slot: sub 1 stores five (the crate's STATUS_EVENT_CHOCOBO), sub 7 stores eighty-five
(STATUS_EVENT_MOUNT) and additionally writes a fetched byte *incremented by one* to a per-entry
byte — kuluu models that as its mount cue carrying `work + 1`; subs 0/5/8 store zero (idle), sub 8
also zeroes the same entry byte. Sub 3 stores five, calls the entity method with argument two, then
stages the fetched words at operand offsets six through fourteen into a block whose first element
gains thirty-two before an entity call; sub 6 is that body's wider twin also consuming the next
WORD operand (widths sixteen/eighteen).
Sub 2 waits until the entity's visual object reports ready through its readiness method: not-ready falls
to a bare return with *no advance* — the retail park; kuluu takes the ready branch. Sub 4 is pure
advance (nothing but the width tail). Kuluu's existing armed arm reproduces every observable case
(ledger closed to MATCH this pass); the staged blocks behind subs 3/6 name an entity call whose
callee was not opened — parsed-not-applied half, named here and in Provenance. Corpus census from
the gap table: rare enough never to appear in the sweep's refused list.

## Friend-pass send/poll pair: 0x87 / 0x88 (2026-10-29 late)

Identical twins except the payload word each send case stores into packet id twenty-seven: 0x87
sends zero (sub 0) or two (sub 1); 0x88 sends one (sub 0) or three (sub 1). Every send case then
arms a shared in-flight byte global, advances exactly two, and yields; if the packet allocation
fails instead, only the yield byte is raised with no advance — the instruction retries next pass.
Sub 2 on both opcodes polls that latch: clear advances two, still-set spins without advancing (the
server's reply handler clears it). Any other sub spins pending-only. Kuluu refuses both pairs by
declared posture (test `unsupported_query_workflows_do_not_send_or_fabricate_results`) — one opcode
earlier than retail parks; the ledger rows carry the law so an arming writer step has the spec:
widths two everywhere, latch global named in Provenance, and the reply-clearing packet unread.

## Scripted movers armed end to end: 0x31 CodeSMOVE / 0x5A CodeMOVE2 (2026-10-29 late)

Both handlers read to the last instruction: they are a stage/poll pair that walks an entity without
any server or mover-system involvement.

**Stage (sub 0).** The target comes from the block's own event-entity word. Retail first releases the
target's two visual animation channels, then fetches three position WORDs through the operand
redirect into a per-slot 32-byte mover block (x/y/z at +40/+44/+48 after the block, each scaled by
one thousandth), and CodeSMOVE additionally stores one more resolved WORD — signed for all later
reads — as the move's **MoveTime** budget; widths ten and eight respectively.

**Poll (sub 1, width two).** While a move runs the handler re-executes every interpreter pass: it
steers heading to atan2(goal−current) into entity float +340, integrates current floats toward the
goal by one frame's arc (elapsed-frame factor × the mover's own speed word × a fixed constant), and
— when that single step still covers the remaining distance — snaps exactly onto the staged triple.
Advancing past the opcode happens on snap, or for CodeSMOVE also once its MoveTime budget counts
down to zero: a walk longer than the budget lets later script beats race ahead of the body. A
budget already zero at stage time skips all integration — no move ever starts. Every poll pass marks
word +300 bit 17 before yielding. After handling, both run a ground-height tail through the scene
singleton (probe the triple, replace float +348 when it answers) unless entity flags say another
system already owns the placement; kuluu's renderer movers place and ground themselves, so that
tail has no counterpart to build.

Kuluu arms both opcodes in the base VM (vm.rs mover arm): case 0 issues the walk cue and CodeSMOVE
self-arms its budget as a timed move-hold, decaying with the same clock all holds use; case 1 parks
while any walk hold runs, clamping that hold down to the budget at every poll pass so expiry lands
on the authored number however a host sized it, publishes bit +300/17 in the entity-flag cue space,
and takes its two bytes on arrival or expiry — first wins. Unknown subs halt (widths absent),
matching the handlers' no-advance tails. The corpus effect is measured: the sweep's refused counts
for these two opcodes (sixteen and eleven events) fell to zero and total ended events rose by the
same twenty-seven, with cancellations, spins and step limits unchanged.

## Item-card switch and pair dispatchers audited: 0x93 / 0xAD / 0xAE (ledger closes, 2026-10-29 late)

The corpus's three biggest NO-ARM rows by usage were read to their last instructions — the width-three
item-card op thirteen thousand four hundred fifty-four times over one hundred forty-seven zones, and the
two pair dispatchers. Laws:

- **0x93** (`f_100b9d30`, width three): reads a word operand immediately after the opcode. Zero takes the
  close path: the menu handler's staged record (its global object plus eight) goes through a singleton
  helper that sets an init-once guard byte, short-circuits into a park byte when the record sits in mode
  fourteen, and otherwise clears its in-progress flag before handing it to a five-argument staging call;
  then the menu handler object itself is reset whole — state fields zeroed (window-bounds word,
  pending-counter group) and its nine-dword sentinel-filled tail array restored. Non-zero takes the open
  path: the operand is passed as an item id into **the mode-0 opener already named twice over** —
  Not established #11 above and the 2026-10-15 menu-docking record, whose RVA list quotes these exact event
  handler call sites. Both paths advance three; nothing touches the yield byte. Kuluu has no event-driven
  item-card surface, so the width-three skip stays faithful; the law above is the spec-away arming brief.
- **0xAD** (`f_100bb340`, always twelve — every exit including gate failures lands on the advance-twelve
  tail): resolves two actor operands (operand dwords at +4 and +8 through the fetch/resolver pair),
  re-validates each against its table slot's staleness token pair (+116/+120 must still match what was
  fetched), demands bit 9 of **both** actors' first-flags eligibility dword, then takes a nibble from byte
  +1 (zero..nine) selecting one of ten dispatch entries on the *first* actor's handle object ([entry+160]);
  every call carries the four-character module literal naming its main script plus the operand word at +2,
  and one entry pre-pushes two zero arguments. The ten entries' semantics sit below a handle interface
  kuluu does not model (Not established #12).
- **0xAE** (`f_100bb620`, sub byte +1 cases zero..eight): observed effect families are the width-six case
  staging two fetched words into one singleton helper (the weather law of the parsed doc lineage; callee
  unread), a resolved-entity family storing either the word fetched at +6 or the constant twenty-three
  into entity field +472 (an inner byte test picks between store forms), and pair cases that set or clear
  an actor link slot: one handle object's field +1944 receives the other operand's [entry+160] handle, or
  zero on the clear case (single resolved operand, token at +2). Case widths six/eight/ten were verified
  equal to kuluu's parsed rows on both failure and success paths; unknown subs spin in retail and halt in
  kuluu as elsewhere.

## Fourth flags word: force-setter family armed on a new cue base (2026-10-29 late)

The four adjacent entity flag dwords (+288, +292, +296, +300) now each own a kuluu cue bit space:
`ENTITY_FLAG_BASE`, `ENTITY_WORD_124_BASE`, `ENTITY_WORD_128_BASE` and — new this pass —
`EventCue::ENTITY_WORD_12C_BASE` at 0x80 (`ffxi-event/src/cue.rs`). The fourth word is the one the
turn-wait handler gates on: `f_100b7dd0` parks (yield byte set) while its **bit 1** stands, releases and
advances otherwise — confirmed instruction-by-instruction this pass. Three force-setters armed onto it:
0xA4 pins bit twenty-six and 0xA5 bit eleven to the low bit of the byte at +1; 0xC0 (width three) applies
the identical law to its fetched word's low bit — the same byte, since little-endian words carry their low
bit first. None of the three fetches an actor operand: they read the current event entity id from the
interpreter's own stream state (a word at offset two), which is exactly `ActorLookup::EVENT_ENTITY`. The
null-table-slot write skip folds into kuluu's publish-anyway posture as with every sibling op. One
comment correction rode along: the mover arm had described its in-flight bit seventeen as a +300 write;
retail ors it into the **second** word ([eax+292], quoted under Provenance) — the cue law was always
correct, now the comment says so too.

## Small handlers audited: 0x95 / 0x96 / 0x97 / 0xA8 / 0xD3 / 0xC2 / 0xC3 (ledger census tail, 2026-10-29 late)

The remaining zero-or-low-use NO-ARM rows, each read to its last instruction:

- **0x95** (`f_100b9c20`, width three): when the current event entity's table slot exists, two methods
  run on it — a word operand at +1 feeds the first; the second is a no-argument refresh tail shared with
  its sibling below. Advance past happens whether or not the slot exists.
- **0x96** (`f_100b9c70`, width one): the same pair with no operands (the ledger's earlier "none" exec
  form was this repo classifier's known blindness to `inc` advances — the increment sits at its tail).
- **0x97** (`f_100ba0c0`, width five): after a preparation helper, two words fetched at +1 and +3 are
  stored as dword fields (+312 / +316) of the window singleton — the same global the selector op's
  case-two dispatches through.
- **0xA8** (`f_100b9780`, width six, sub byte one or two only): two fetched words at +2/+4; a widget-
  singleton null gate covers everything. Case two first stages an indexed entry with the first operand
  plus a sentinel triple and finishes by closing the map window through the close-map sibling already
  named for 0x8A's audit row; both cases then write the second operand into the singleton twice, one
  pass accompanied by a literal name string.
- **0xD3** (`f_100b7140`, running case width six): only a zero sub byte runs — every other selector exits
  without touching exec (retail spins there). The running case resolves one actor through the standard
  pair, demands its eligibility bit, asks a predicate of the handle against a class-table pointer, and on
  true calls one more handle method. **Armed this pass** (`OP_STAGED_RECORD_PROBE_D3`): halt on non-zero
  sub reproduces the spin law even though no authored script uses the op; running-case width pinned in
  `sub_width_opcodes_advance_by_their_case_width`, spin-halt in
  `sub_width_opcodes_stop_rather_than_falling_back_to_the_fixed_size`. Before this arm kuluu skipped six
  over a non-zero sub — latent, never exercised (zero uses across the corpus).
- **0xC2** (`f_100bbf80`): two query forms storing a global-state answer into work slots through the
  registry-store helper family — sub one queries and takes four, sub two feeds its fetched word at +2
  into the other query and takes six — and any other selector lands on a jump-set that moves exec
  exactly two. Un-armed: both helpers' answers are unread client state, so the work-slot stores cannot
  be produced without fabricating values; widths recorded here (four / six / two).
- **0xC3** (`f_100bc000`, width seven): an index word at +1 must sit in zero..seven or the handler spins
  without advancing. The slot is a five-dword entry of a global staging table; four dwords come from a
  helper-named block (selector three) and the fifth stores the word fetched at +5. Un-armed — pure
  client-global staging with no kuluu consumer, zero authored uses.

## Census tail two: registry lookups, widget twins, clock reads (2026-10-29 late)

The next slice of the census's remaining NO-ARM rows, each read to its last
instruction. Two of them carried latent over-skip bugs (kuluu skipped past paths where retail never
advances at all) and are armed here with halt-law-correct arms — joining 0xD3 from the section above,
that closes three such latents this pass; the rest already width-matched and stay audits.

- **0xBF** (`f_100bbd80`) — registry lookups, **armed width-only**. A selector byte maps through the
  handler's own sixty-four-entry byte table onto jump-table bodies; only three selectors ever advance:
  zero fetches one dword field of a global registry (width eight), thirty-two and sixty-four index
  twelve-byte-entry registries with the word fetched at +4 (width ten). Every other selector — inside or
  above the ceiling — lands on an exit that pops without touching exec: retail spins there, kuluu halts.
  Out-of-range entry indexes emit literal-string trace calls before storing zero; the registry getters,
  trace literals and both store slots are under Provenance. Before this arm kuluu skipped a flat ten
  over every spin case.
- **0xB1** (`f_100bba70`, width four) — mode-bits writer, **armed**: selectors zero/one/two store bits
  one-two, bit zero, or a constant of the shared mode dword (the same global at Provenance that 0x9C
  consults; its producer remains unnamed) into work slot two; every other selector exits without
  advancing — retail spins, kuluu halts. Values unpublished: no source names their consumers.
- **0x84** (`f_100b9100`, width one) and **0x86** (`f_100b8f30`, width six) join the fourth-flags-word
  family, **armed**: 84 ors bit zero into `[entry+300]` of the current event entity with no operand test
  and no clear branch; 86 is the resolved-actor form — actor token at +2 behind the standard eligibility
  gates, force-stores bit three from the byte at +1. Both cue in `ENTITY_WORD_12C_BASE`; tests
  `entity_word12c_bit0_op_is_an_unconditional_set` and
  `entity_word12c_bit3_resolved_form_names_its_actor_and_low_bit`.
- **0xB9** (`f_100b9820`, width eight): the three-operand sibling of A8's widget pair — fetches +2/+4/+6,
  same singleton-null gate, case two stages and closes the map window through the close-map sibling
  already named for 0x8A. Un-armed (widget effects below kuluu's boundary); widths equal so no latent skew.
- **0xAA** (`f_100babe0`, width seventeen): seven field getters over an operand-named record; the first
  result is offset by eight-hundred-eighty-six, two more are incremented, and all seven land in odd work
  slots three through fifteen. Likely an examine-record reader given the 0xCC overlap; getters unread
  below call sites.
- **0x57** (`f_100b0350`, width three) and **0xAF** (`f_100bb9c0`, width eight): value reads through the
  shared coprocessor-conversion helper — 57 adds a converted delta to its fetched word into work slot
  one; AF reads three floats from a named singleton's fields (three-field triples at +68..+76 or +80..+88
  by selector) into work slots two, six and four. Un-armed: values below the cue boundary.
- **0xBE** (`f_100b3e20`, width three): reads a dword from exec's own thirty-two-byte slot array indexed
  by its child-counter field (+2 as authored) into work slot one — an internal child-slot read; law only.
- **0xD9** (`f_100b7be0`, width two) and **0x9E** (`f_100ba8a0`, width two): store "operand equals zero"
  to one client global each (addresses under Provenance); kuluu models neither, so the width skip is the
  whole observable law — same posture as the 0x30/0x2E audits.

### Queue closed: the last never-read handlers, three owed audits (2026-10-29 late)

The ledger queue's final five never-read handlers (0x04 / 0x91 / 0xB7 / 0xD8 / 0xA2), each
dumped and read to its last instruction, plus three audits owed from the previous batch
(0x83 / 0x85 / 0x9C). Two of the five carried latent over-skips of the AE/BF class — kuluu
consulted no widths for them at all (no arm), so the generic skip fell back to each op's
declared fixed size while the handlers' own advance sites say otherwise per case — and are
armed width-only this pass; their `opcode_meta.rs` rows already encoded the faithful widths,
they were simply never consulted.

- **0xB7 (`f_100afee0`), mover-bridge writer, armed width-only.** A selector byte maps through
  five jump-table bodies (table dumped from the image). Case zero resolves an actor and, behind
  its mover-block gate, stores a fetched stream word into that mover's own work-slot array —
  the same store helper event scripts use for their own slots, aimed at the mover block; every
  path through this case advances ten, gates included. Case one copies the resolved record's
  two-word name pair when its count field is positive, else a sentinel literal pointer, into
  that mover's work-slot pair; cases two and three store (respectively) a fetched dword behind
  an eligibility test or zero, and the resolved word or a 0xFFFF sentinel when the lookup fails;
  case four clamps a fetched index to zero..eight and writes one entry of an eight-entry global
  word array, out-of-range emitting a trace literal first — each of cases one..four advancing
  uniformly eight on every path. Selectors above four jump to a shared epilogue that pops with
  no exec write: retail spins there; before this arm kuluu skipped a flat ten past those cases.
  Widths as recorded in the meta row unchanged — pins added from the handlers' own advance sites.
- **0xD8 (`f_100bc060`), staged-block writer, armed width-only.** Resolves one actor behind the
  standard gate pair and demands its mover block; a five-entry jump table selects bodies that
  all write three consecutive fields of that mover block (plus a fourth field beside them).
  Case zero copies four raw dwords from the resolved record unchanged (advance six — the advance
  register is loaded at entry); cases one/two/three each fetch one stream word, scale it by the
  send pair's heading constant pair (the author-rounded circle literal times exactly
  one-four-thousand-and-ninety-sixth) and store one field each (advance eight, per their own
  register loads); case four chains all three fields (advance twelve). Above-selector values —
  and every unresolved-actor gate failure — land on the shared tail advancing six, so kuluu's
  pre-existing default row (six for unlisted selectors) matches retail's fall-through exactly.
  Named divergence: a gate failure inside a live case advances six there where kuluu steps that
  case's width — same posture the 0x7A family takes. The linear-walk gap census sees 124 sites
  over 24 zones, and its operand samples are exactly width-eight records carrying selector bytes
  one/two/three — i.e. real authored data sits in the cases where kuluu's old fixed-twelve
  fallback stepped four bytes too far; whether any of those 124 sites lie on currently driven
  paths is unmeasured (the sweep posture was identical before and after arming).
- **Audits, no change.** **0x04** (`f_100b0120`): one instruction — exec plus three, nothing
  read, nothing stored (the MATCH-ADVANCE verdict stands). **0x91** (`f_100b9b70`): fetched word
  at +1 multiplied by exactly ten and stored as a float field of the current event entity
  (+364), width three; readers of that slot unnamed (Not established #14). **0x83**
  (`f_100b90e0`): getter `f_101039e0` — a counter call plus one global addend, the same shape as
  the first getter of AA's family — stored into work slot one; width three. **0x85**
  (`f_100b9120`): operandless singleton call with argument zero; the callee tests a display-state
  global and posts literal-named flags to a UI manager object (all below kuluu's boundary);
  width one — another `inc` tail the ledger classifier reads as "none". **0x9C** (`f_100ba100`):
  reads only mode globals, never the stream: from the shared mode singleton it combines a
  signed state byte with its dword+8 escalation (state negative and that field two or three →
  one), then a decrement ladder stores constant pairs {2,1}, {3,1}, {4,1} or default {1,1} into
  work slots one and two; every exit width three. **0xA2** (`f_100b4970` → helper `f_100b4af0`,
  read this pass): the third magic-pair handler — same selector space as 0x73/0xC4 (legacy
  notes cite sixteen..nineteen; this wrapper passes its own value). The helper re-verifies two
  resolved actors are still each other's pair (their identity fields must match across the two),
  demands both eligibility bits, then tests whether the authored dword at +11 overlaps a range:
  the special value thirty-seven-thousand-seventy-four first takes the operand through the
  scheduler-id bucket anchors into one scheduler-entry loader. On "no overlap" execution does not
  advance — exec sets its pending flag and the thread re-runs the instruction each pass, i.e.
  retail *parks here until the two actors separate*; every other outcome advances exactly
  fifteen. Kuluu stays un-armed: its generic width-fifteen skip equals every failure path but
  cannot reproduce the conditional park (the pair identity re-check and overlap test read live
  client state). The linear-walk census sees thirteen sites over seven zones; sampled records
  carry special-form actor tokens and inline four-character tails. Divergence at those sites
  requires the predicate to answer "separate" — unmodelled, named.

### The two faithful arms whose handlers were still unopened: 0x7A / 0xB5 (2026-10-29 late)

Two WIDTH-ONLY rows carried "handler not read" notes from an earlier pass; both bodies are now
read to the last instruction, jump table dumped. kuluu's encoded widths equal every case advance
site for site — nothing in its behaviour changes, and five pins were added so the confirmed
widths and spin laws cannot regress.

- **0x7A (`f_100b3a50`), six cases under a selector ceiling of five.** Cases zero and one resolve
  an actor and hand it (with token −1 / plus a stream byte) to the eligibility-chain helper —
  advance six / seven. Case two is a **child attach**: behind its gates it sets the thread's
  current-entity word to the fetched id, stores the second fetched dword into the thread's operand
  slot, points the thread's mover-state field at that actor's mover state block, and increments
  that mover's nesting counter; exec's pending flag rises on **every** path — a gate failure
  raises it with no advance (retail re-dispatches). Case three is the twin **detach**: decrement-
  and-clamp the same nesting counter on the current entity's mover, then restore the thread's
  execution fields from its own saved pair slots — advance two, pending set. Which code fills
  those saved slots is interpreter machinery outside this handler (named in Not established).
  Cases four and five are predicate parks: a fetched operand tuple goes to one of two query
  helpers; answer two raises the pending flag without advancing (kuluu halts there), every other
  answer advances eight / six. Selectors above five pop with no exec write.
- **0xB5 (`f_100b10f0`), name-patch op.** Sub zero resolves the actor named at +2 and, when that
  record's name count is not already set, copies a six-dword literal block — addressed *by a
  fetched stream dword* — over the record's 24-byte name area and clears its terminator byte;
  advance four on every path, null-slot writes skipped but advancing. Any other selector sets the
  pending flag with no advance: retail re-dispatches until something changes, kuluu halts.
  The pointer-valued operand is an authoring-time literal-block patch; it cannot be authored in
  kuluu's event data model, so the parsed-not-applied posture stands — now on read handlers.

### Ledger reason-close pass: every open row carries its recorded reason (2026-10-30)

The ledger's job is one verdict plus a reason per opcode, and 23 rows still read as the bare
classifier cell ("no constant, no arm") after their handlers had been audited — closed rows
looking open. Three changes:

- The classifier now prints an AUDITED row's reason in the notes column instead of appending it
  to the handler-form column, so every non-MATCH row's verdict is explained where readers look;
  machine facts (multi-path yields, helper chains) still follow beneath it.
- Eight further handlers were read to their last instructions and staged as audit rows:
  `f_100b4970`/`f_100b4af0` (A2 — the intersection call at @100b4be8 arms the pending byte with
  no advance on success; gate failures take the shared advance-fifteen tail),
  `f_100b9780` (A8 — map-singleton marker pair, close through the sibling named for 0x8A's
  audit row, advance six everywhere including when the singleton is null),
  `f_100babe0` (AA — one fetched word feeds seven getters into work slots three through fifteen
  at stride two, first getter carrying a constant 886 addend; advance exactly seventeen),
  `f_100bb9c0` (AF — selector zero stores the singleton object's float triple at dword fields
  +80/+84/+88 through coprocessor-conversion calls into work slots two/four/six taking eight,
  selector one the same triple at +68/+72/+76, every other selector returns without advancing —
  retail spins there while kuluu's declared-eight skip walked on — **armed width-only** (new
  `sub_size` row {zero/one → eight}, unknown selectors halt through the arm), pinned by
  `work_slot_triple_advances_eight_on_both_selectors` plus a stop-list pin; zero authored uses in
  this corpus, so the arm is posture-neutral and cannot break a scene),
  `f_100b9820` (B9 — A8's three-word sibling under the same map singleton, advance eight
  whether or not it exists), `f_100b3e20` (BE — copies a dword from the thread-slot array at
  stride 32, slot ([exec+600]+2)+1 field, into work slot one plus literal one into slot two;
  advance three; zero authored uses), `f_100bc000` (**armed this pass**, control-flow only: the
  decision needs no client value, only the operand). Its resolved operand must sit in 0..7 or the
  handler returns with no advance — retail spins, and kuluu's flat seven-byte skip walked past
  that. The first arming attempt read the operand word raw and refused seventeen events retail
  runs: every authored site in this corpus carries it as a reference-window record (`C3 00 80 …`,
  raw +1 word `0x8000`), so it resolves through the redirect kuluu's work-operand getter already
  implements (`f_100af4b0(N)` wrapping that getter, per this record's scheduler-id section) and a
  plain read returns a negative. With the operand resolved properly the whole-corpus posture is
  identical to before the arm — every authored site resolves inside the table — while the spin law
  is now pinned for data that breaks it (`staged_slot_loader_halts_outside_its_index_table`). The
  staged five-dword global table and the fetched blocks stay below kuluu's cue boundary, so only
  control flow is applied), and `f_100b7be0` (D9 — stores whether its operand
  word equals zero into a fixed client global and advances two; audited MATCH on the
  effectless-global-poke posture of 0x30/0x2E/0x9E).
- Batched audits read in earlier sections (0x57, 0x83, 0x85→MATCH, 0x91, 0x95, 0x96, 0x97, 0x9C,
  0x9E→MATCH) and the two zero-use default-selector rows 0x04 (empty advance-three stub, MATCH
  on the 0x6D posture) and 0xC2 entered the AUDITED dict. Reading `f_100bbf80` to its last
  instruction also **retracts** what this pass first wrote about 0xC2: it called the default
  selector a latent over-skip, but kuluu's generic skip already carries the sub-selector through
  its width table (`vm.rs` `op_width`, whose own comment states that rule for exactly these ops),
  and the handler confirms the table — the default selector adds two @100bbfa3. No arm needed;
  row corrected rather than left asserting a gap that does not exist.

Census after this pass: MATCH 184, NO-ARM 20, WIDTH-ONLY 15; every non-MATCH row carries a
recorded reason (a checker asserts no thin cell remains). Two closes are code, not bookkeeping —
the float-triple op and the staged-slot loader joined the armed width-only family — and each was
re-measured against the whole corpus: ended 60830 / cancelled 1772 / refused 107 / spun 9 /
step-limited 84, identical before and after both arms (the intermediate refusal count of 124 is
the reference-window mistake above, caught by re-measuring rather than by reading the code). Unit
tests: 254 green — two new (`work_slot_triple_advances_eight_on_both_selectors`,
`staged_slot_loader_halts_outside_its_index_table`) and one extended stop-list pin. Embed
regenerated and `--check` silent.

### The derived opcode effect table: every slot, its own instructions as the spec (2026-10-30)

This record's queue was hand-curated, which is exactly how "no arm" and "effectless" survived as verdicts.
The per-opcode effects are now **derived instead of asserted**: `scripts/opcode-effect-table.py` walks
FFXiMain's event dispatch table (the same slot parsing the parity ledger uses), resolves one-jump thunks,
and emits `references/data/opcode-effect-table.md` - one row for every dispatch slot, listing how that
handler steps past itself, whether it writes the yield byte, **every memory store it performs** (classified
as a fixed client global, an interpreter field, or a base-register-relative write), every engine call it
makes, and any jump table it branches through. Every entry carries its instruction VA.

What that says about the corpus: 219 slots; 133 handlers contain stores, 193 contain calls, 23 branch
through a jump table, one slot is a thunk whose effects live downstream (the row names both hops). Exactly
**two** handlers in the entire dispatch neither store nor call - opcode 4 and opcode 6D, both verified by
hand against their bodies. That set is pinned in the script, and `--check` fails if any opcode written off
as effect-free turns out to write or call something.

Gate: `scripts/checks.sh opcode-table` regenerates the table and refuses when the committed artifact has
drifted or the effect-free set changed; wired into `.github/workflows/ci.yml` (after `records`) and
`.githooks/pre-push`. Consequence for this record's queue: an unmodelled row can no longer be argued out of
existence. Its derived effect list is the specification, and the only legitimate status for a row is
"retail does X; kuluu does not do X yet".

### The "effectless" rows were not effectless (retraction; three stores armed, 2026-10-30)

Six ledger rows read MATCH on the reasoning *the handler writes only a fixed client global kuluu does not
keep, so stepping past it is the whole observable law*. That is not a verdict — an opcode that stores has
to perform the store, and whether anyone reads the cell is beside the point. Re-reading all six handlers to
their last instruction: **two** are genuinely store-free, **three** write client state (now implemented), and
**one** calls into the UI layer and was mislabelled outright.

| op | handler | what it actually does | kuluu now |
|---|---|---|---|
| 0x04 | `f_100b0120` | the body is a single step-past of its own record and nothing else — no store anywhere in it | width skip is correct: there is no effect to model |
| 0x6D | `f_100b7850` | same shape with the wider record; zero stores | width skip is correct |
| 0x30 | `f_100b5620` | zeroes the fixed client byte at image address A (see Provenance), then advance one | **armed** as `OP_CLIENT_BYTE_10488100_ZERO`, writing `EventClientGlobals::byte_10488100 = false` |
| 0xD9 | `f_100b7be0` | reads its inline operand byte, forms "operand equals zero", stores it as a byte into fixed client cell B, advance two | **armed** as `OP_CLIENT_BYTE_10463924_OPERAND_IS_ZERO` |
| 0x9E | `f_100ba8a0` | same predicate stored as a full dword into fixed client cell C (register pre-zeroed, so the value is exactly one or zero), advance two | **armed** as `OP_CLIENT_DWORD_104898FC_OPERAND_IS_ZERO` |
| 0x85 | `f_100b9120` | calls `f_101d6110`, which posts to the UI singleton through method `f_1015e350` with one of three **named menu windows**; this record previously said "nothing stream-visible", which is wrong | ledger row downgraded to NO-ARM; law recorded, arm owed |

The two cells op 0x30 and 0xD9 write are live state, not scratch: each has engine writers on one side and a
consumer on the other (all sites in Provenance), which is how they got named as much as the image allows.
None of these three ops produces a cue — retail keeps the values client-side — so kuluu exposes them through
`EventVm::client_globals()` rather than inventing a host event for each write. Implementation and pins are in
the delta table below; the whole-corpus sweep posture is unchanged by the arms (ended 60830 / cancelled 1772
/ refused 107 / spun 9 / step-limited 84, identical before and after), because the widths were already right
and only the store was missing.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- |--- |
| the opcode-to-behaviour mapping is fixed inside the client image, not supplied as data | `ffxi-event/src/vm.rs` arms each opcode by hand rather than mirroring a table | matches in effect — what it buys: every kuluu arm has one exact client-side counterpart to diff against, and 65 opcodes have no name at all in the crate (section above; regenerate with `scripts/opcode-dispatch-table.py`) |
| Set-volume: channel mask (low byte of the work operand), full/silence from an inline flag, applied per selected mixer arm without ramp | `ffxi-event/src/vm.rs` `OP_SET_SOUND_VOLUME` → `EventCue::SoundVolumeSet`, routed by kuluu-session to `kuluu-render/src/audio.rs` bus scales | matches for the audible law; retail's reciprocal write into its global multiplier slot is folded into both kuluu buses (bit 16), stated above |
| Change-volume: per-mille target through the change arms | `OP_CHANGE_SOUND_VOLUME` → `EventCue::SoundVolumeChange` | differs knowingly — kuluu applies the thousandths-scaled target at once; retail's mixer-side second operand (candidate fade time) is unread, see Not established |
| event-scoped restore of both bus scales when the cutscene session ends | `kuluu-render/src/audio.rs` (drain + `CutsceneEnded` path) | matches — mirrors the 0x5D duck convention |
| second door pair write the second door status slot on the block's own event entity | `OP_DOOR_OPEN2`/`OP_DOOR_CLOSE2` → `emit_status_event_cue(STATUS_EVENT_DOOR_OPEN2/CLOSE2)` | matches — render-flag gate unmodelled exactly as for the first pair, now commented at both |
| prop-label layer state reaches the host and persists while events run | `OP_OPEN_MAP_PROPS` → `EventCue::MapPropsState` → `event_map_sync_system` (`kuluu/src/view_native/text_input/event_map.rs`) stores the pair on `MapScreenState::event_props_layer` (`kuluu-render/src/hud/map_screen.rs`) | differs — kuluu has no prop-label layer to show; state stored, nothing drawn (blocker named above) |
| the second marker opcode places an identical marker and only its two helper-only words go unused | `OP_MAP_ADD_MARK` emits the existing `EventCue::MapMarker` with its own operand offsets (name inline at +11, underscore rewrite in the VM) | matches for every host-visible field; window-state helper branch has no kuluu counterpart |
| per-actor flag writes parsed, lookup-resolved, published; nothing applied | 0x92/0x94/0x81 arms → `EventCue::ActorRenderFlag` (bit index from the image offsets) → session `AgentEvent::ActorRenderFlagChanged` | missing host effect by decision — no source names the bits' visual role; repo precedent for 0x90's unnamed flag half applied deliberately |
| map/zone reload spreads over three scheduler deliveries, holding twice, reloading and advancing on the third; the pair differ only in the pending branch closing things | `OP_MAPLOAD`/`OP_MAPLOAD_KEEP` → `EventVm::mapload_three_pass_hold` + `EventCue::MapReload { map_id }`, host reopen in event_map.rs | matches for the cadence and the not-pending branch (the only branch kuluu can observe); pending-streaming branch never taken, recorded above; retail keeps its counters across threads where kuluu keeps them per running VM — a difference no authored script is known to expose |
| music-read yield holds while a read runs; gates now fully named (song-table compare pair + pending-counter busy test, every pass yields) | `OP_MUSICREADWAIT` → single-pass hold on the VM | differs knowingly — predicate is loader-internal client globals with no kuluu counterpart (named this pass); one held pass stands in for "while loading", named above |
| mount-status switch: widths 6/16/18/8 from both jump tables, triple gate failing to the width tail, status stores zero/five/eighty-five into the door-status dword plus the inc'd fetched byte (mount id) at one entry byte, one ready-park body that spins without advancing | `OP_CHOCOBO` arm + `sub_size` row; mount cues carry STATUS_EVENT_CHOCOBO/MOUNT/IDLE and the work+1 mount id (tests incl. `chocobo_rental_emits_its_staging_cues`) | matches for every observable case (ledger closed this pass); subs 3/6's staged-block entity call unread below the handler — parsed-not-applied half named above |
| scripted movers: stage issues walk + budget (widths ten/eight), poll parks while a walk hold runs, clamps the hold to the MoveTime budget every pass so expiry lands on the authored number, advances two on arrival-or-expiry with second-flags-word bit 17 ([entry+292]) published — an earlier
  comment calling it a +300 write was wrong, retail's or-in sits at [eax+292] (quoted this pass); staged
  budget zero means no walk at all | `ffxi-event/src/vm.rs` mover arm (`OP_SMOVE | OP_CODE_MOVE2`) over `move_holds`/`hold_move`, budget field decaying in `tick`; tests `smove_stage_arms_the_walk_and_its_move_time_budget`, `smove_poll_expires_mid_walk_at_the_move_time_budget`, `smove_with_a_zero_move_time_stages_no_walk`, `codemove2_pair_parks_on_the_walk_and_halts_on_unknown_subs` | matches for the observable pair — sweep refused counts 16+11 fell to zero, ended rose by 27 with no other posture change; retail's per-float march is kuluu's renderer mover (cue-equivalent), and the ground-probe tail plus animation release are parsed-not-applied halves named above |
| friend-pass pair: packet-27 sends with payload words 0/2 and 1/3, shared in-flight latch armed on send and cleared by the server reply, poll advancing only when clear, width two on every case, spin-on-set matching kuluu's halt shape one opcode later | no arm — refusal (`unsupported_query_workflows_do_not_send_or_fabricate_results`); `sub_size` rows equal for all three subs | differs by named decision — refuse rather than send-and-park an unread reply packet; law recorded above so arming is a spec-away writer step |
| display/flash colour stages classify with their measured destination triples instead of being unknown-stage skips | `ffxi-dat/src/scheduler.rs` raw 0x51 / 0x72 → `StageKind::DisplayColorDrive` / `FlashColorDrive`, payload via `ScreenColor` | matches for classification; rendering still missing on the composite-order blocker carried from 2026-10-28-cutscene-scheduler-stage-tags.md (rows there re-scored to cite this reading) |
| map sub-op family: any-window-open gate yields without advancing (unreachable for kuluu); widths 4/2/2 taken from the handler's own four advance sites because its fetch helper never moves exec; case 2 re-bases six back to read the previous instruction's last WORD into a packet-242 request, net +2 on accepted send and a yield-byte spin on refused | width-only advances for `{0: 4, 1\|2: 2}` via `opcode_meta.rs` `OP_LOADROOM` row through the shared sub-byte family fallthrough; widths pinned by `sub_width_opcodes_advance_by_their_case_width` (subs 0x01/0x02 added this pass), unknown-sub halt pinned by `child_stopped_on_unrunnable_opcode_is_dropped_and_releases_the_reqwait` (an 0xFF selector) | matches for every width and the halt; window-present effect and packet-242 request parsed-not-applied — named missing counterparts, refused-send spin unreachable without them |
| random draws (plain and range-wrapped), heading-unit sine/cosine stores, vector-to-heading atan2, guarded remainder | `ffxi-event/src/vm.rs` `OP_RANDOM`, `OP_RANDOM_MODULO`, `OP_SINE`/`OP_COSINE`, `OP_VECTOR_TO_HEADING`, `OP_MODULO`, LCG state on the VM; tests `random_stores_follow_the_crt_sequence`, `random_modulo_zero_divisor_stores_zero_without_a_draw`, `trig_stores_round_to_nearest_at_pinned_headings`, `vector_to_heading_pins_the_four_cardinal_sides`, `modulo_guards_both_operands_like_div` | matches — widths, operand offsets, sign flips, no-draw guard and rounding all lifted from the handlers; float-transcendental bit-parity with x87 is beyond any host library and is not claimed (pin values sit far from ties) |
| END's two dispatch twins end the current event thread | `OP_END_TWIN_CA`/`OP_END_TWIN_CB` share the explicit-END arm | matches — refused (`Unimplemented`) before this pass, which cancelled corpus events mid-cutscene on any CA/CB byte; ledger-audited row |
| bit-array clear: flat index at operand 3, bound at 5, read-modify-write of the operand-1 word with one bit cleared, advance seven | `OP_BITARRAY_CLEAR` arm; test `bitarray_clear_mirrors_set_addressing_and_guard` (sets then clears, pins the bound-refusal leaving the bit set) | matches |
| entity flags-word twins: bit := inline byte low bit at `[entry+288]` bit 19 / bit 21, lookup twin like 0x92, self variant on the block's own entry | `OP_ENTITY_FLAGS_SET_BIT19`/`SET_BIT21` arms → `EventCue::ActorRenderFlag` in the `ENTITY_FLAG_BASE` bit space; test `entity_flag_twins_carry_first_word_bits_and_low_bit_values` (pins both values of the low-bit law and both widths) | matches for parse, width and value law; host effect missing by decision — no source names either bit (same posture as 0x92/0x94 above) |
| nine-case entity turn/move speed switch: per-case widths in `sub_size`, custom-slot/lookup forms parsed, case 5 publishes the bit-21 flag cue from its +6 value byte, cases 6-8 advance their case width | `ffxi-event/src/vm.rs` dedicated `OP_ENTITYSPEED` arm (replacing shared width-only handling); tests `entity_speed_advances_by_the_case_width_per_the_retail_dispatch`, `entity_speed_bit21_case_publishes_the_entity_flag_at_plus_six` | matches for dispatch, widths and the flag case; custom-slot stores (+220/+224), the render-chain float (×0.1f → +360) and the two visual-vtable handoffs are parsed-not-applied — blockers named above and in Not established |
| work distance: sqrt((G7−G3)²+(G9−G5)²), nearest-even, store @+1, width 11 | `OP_WORK_DISTANCE` arm; test `work_distance_stores_the_rounded_integer_hypotenuse` | matches — pure math lifted operand-for-operand |
| actor distance: lookups @3/@7, squared-sum × 1000.0 nearest-even into @+1, always stores (zero on any failed lookup) | `OP_ACTOR_DISTANCE` arm over `named_actor_position`; test `actor_distance_stores_the_scaled_separation_or_zero` | matches for the raw-position path and the always-store law; kuluu has no interpolated-render source, same named branch as 0x3A |
| turn-wait parks while the entity's `[+300]` bit 1 is up, then releases two visual animation channels ("@tl0"/"@tr0") | `OP_TURNWAIT` arm now parks on the two entity holds the VM can see — a running animation (the 0x9B machinery) and an in-flight authored walk (`hold_move`/`move_holds`) — else advances one; tests `turnwait_parks_while_a_walk_is_in_flight`, `turnwait_parks_while_the_event_entity_animates`; sweep posture unchanged across the corpus | matches for both observable halves of retail's bit (animating, mover steering); turn-only flights remain absent because host facing cues are applied instantly — nothing to gate exists at this resolution |
| thread-slot countdown shared by WAIT/SLEEP/B2 with −1.0 fresh state and strict-below-zero expiry | `arm_wait` clock + `SLEEP_WAIT_UNITS`; tests incl. `a_zero_length_wait_still_costs_a_tick`, `stepping_a_held_wait_does_not_restart_it` | matches per-op; differs only in the cross-op slot sharing kuluu does not model (named above, no authored use sought) |
| heading read: actor lookup @+1 into work operand @+5, wrap [0,one turn), null entry stores zero, ineligible refuses the write | `ffxi-event/src/vm/scene.rs` `OP_GET_HEADING` arm over `named_actor_position` + `wrap_heading_units`; test `get_heading_wraps_the_named_actor_and_zeroes_unknowns` (negative and past-turn wrappings plus the unknown-actor zero) | matches for width, operands and wrap; differs in one named branch — kuluu reads one tracked heading where retail picks between raw `[entry+24]` and an interpolated chain by flags-word bit 7, and conflates retail's two no-data outcomes (null slot → store 0 vs ineligible → no write) into the zero law |
| third-flags-word bit-0 writer: mask-OR store of byte@+1 & 1 (not a latch), advance 2 | `OP_ENTITY_WORD128_SET` arm → `EventCue::ActorRenderFlag` in `ENTITY_WORD_128_BASE` space; test `entity_word128_set_stores_the_low_bit_of_its_byte` | matches for parse/width/value law; host effect missing by decision (no source names the bit — same posture as 0x92/0x94) |
| selector op: widths 4/6/2 from the byte at +1, sel≤1 latch `[entry+292]` bit30 to the selector, sel==2 singleton handoff advance-6 parsed-not-applied, other selectors walk retail's add-2 default | `OP_FLAGWORD_SELECTOR` arm + `sub_size(0x60)` row; tests `flagword_selector_advances_by_the_selector_width`, `flagword_selector_latches_the_word_124_bit_from_the_selector` | matches for dispatch/widths/latch value; the singleton vtable+24 effect unread (Not established) — named, not stubbed |
| word-124 bit-31 latch (low-bit law, width 2), third member of the family | `OP_ENTITY_WORD124_BIT31_LATCH` arm; test `entity_word124_bit31_latch_follows_the_low_bit_of_its_byte` | matches for parse/width/value; host effect unnamed as usual |
| sub-sched 0x5F full law (2026-10-29 revision): sel 0/1 bit-29 latch, sel=0 stores zero, width 2; cases 2-6 advance their case widths whether or not the inline actor dwords resolve (every resolution failure lands on a helper advance tail), parking only at motion-load handoffs kuluu does not model; case 7 parks while either resolved actor reports running and takes its width on null slots | `(OP_SUBSCHED, 0\|1)` family arm + revised `OP_SUBSCHED if sel ≥ 2` arm; tests `subsched_latch_cases_store_the_word_124_bit_29`, `subsched_helper_cases_advance_whether_their_actors_resolve_or_not`, `subsched_case_seven_parks_while_an_actor_is_busy` | matches for widths and both park laws; the bucketed motion loads parse to nothing applied (inputs' identity open, #7) — an earlier width-only row here rested on the retracted rewind sketch |
| third-word bit-17: set-or-clear on whole-byte non-zero (not the low bit), width 6 | `OP_ENTITY_WORD128_SET_BIT17` arm; test `entity_word128_bit17_set_is_non_zero_not_low_bit` (pins 0→false, 1/2/0xFE→true) | matches for parse/width/value law including the non-zero-vs-low-bit distinction |
| empty-stub opcode: handler advances exactly 7 and does nothing else | generic width skip in `vm.rs` (no arm); ledger AUDITED row | matches — there is no behaviour to miss |
| branch compare jump: eleven kinds incl. the AND/OR-zero and bit-subset forms, out-of-range kind takes the branch, target at word +6, fall-through eight | `OP_IF` → `op_if` (`ffxi-event/src/vm.rs`) | matches — both sides read to the last instruction this pass (section above), no code change |
| send-tag pairs: send case sets its in-flight latch and takes width immediately (+2 / +10 for the position pair, floats scaled from +8/+6/+4/+2); poll instance holds until the ack clears the latch or skips when none outstanding | `OP_SENDTAG` / `OP_EVENTPOSSET` arms over `PendingTag` | matches — latch ≡ pending-ack model, spin-case choice already documented at both arms |
| entity-pool branch: present advances past five words, absent jumps to word +3 | `OP_ENTITY_VALID` arm; ledger-audited | matches exactly |
| ext-scheduler load pair: wrapper flags select plain {1,2,7,8} vs TPC-package {0,1,6} accept sets; refusals take the width-advance tail | `OP_LOADEXTSCHEDULER`/`...2` arms (helper law from the Scheduler-helper section) | matches — gates and advance reproduce the helper |
| second door pair: values 45/46 into the block entity's status slot under the gate bit, always one word (`inc`, not `add`) | `OP_DOOR_OPEN2`/`OP_DOOR_CLOSE2` cues; ledger AUDITED closes this pass | matches — as the earlier row said; classifier blindness to increments was why the row sat open |
| hide-always: entity-resolved gate, method call + third-word bit set, advance one | `OP_EVENT_HIDE_ALWAYS` cue arm | matches for the hide law; unnamed bit half unchanged posture |
| animation-yield and zone-read gates: hold on per-entity byte / loader-busy, else one word | `OP_ANIM_YIELD`, `OP_ZONE_READ_YIELD` arms | matches — zone row is retail's not-busy path, stated like the map-reload row |
| range-rect test: model bounds vs singleton regions; hit advances seven past, every miss jumps to word +5 | `OP_RANGE_RECT` arm over `range_rect_hit` | matches exactly |
| clock enable: gated resume + countdown-byte clear, always one word; submap send/poll/write widths 2/2/4 with packet id 235 and the pending byte | `OP_ENABLE_TIMER`, `OP_A6_SUBMAP` arms | matches — a6 under the same ack-pairing law as its A7 sibling |
| window family dispatch: every case width (incl. both inline-string twins at twenty, the poll-without-widget taking two) equals retail's dumped tables row-for-row; only cases 0/1 carry behaviour kuluu models (the work/pending string ops) | `OP_WINDOW` arm + `sub_size` rows in `opcode_meta.rs`; widths verified against the image byte-map this pass | differs by named decision — widget effects (nameplate label, query-window mode byte, value-widget get/store pairs incl. retail's store-zero-without-widget law, box marker) have no kuluu counterpart; laws recorded above so a widget layer has the spec |
| menu family dispatch: unknown subs spin where kuluu stops (both halt progress); string/numeric opener cases latch-and-open, polls refuse while latched and on release take their width with buffer stores; password-pair send case allocates client packet 96 | `OP_MENU` arm + `sub_size`/`is_input_wait`; tested posture `unsupported_query_workflows_do_not_send_or_fabricate_results` | differs by named decision — credential/menu widgets are refused rather than faked; every width verified equal to retail this pass, no SIZE-DIFFER |
| render-flag family: 29 cases of entity-bit pair writes, door-status word stores, one busy-guard pending-byte halt, widths two/four/six; out-of-range subs land on the bare-return spin tail | `OP_RENDERFLAG` arm + `sub_size` row; width pin in `sub_width_opcodes_advance_by_their_case_width` (0x1B→six) | differs by named decision — every case width equals retail's dumped table this pass and the unknown-sub spin matches kuluu's halt; the entity-bit/status-word/window-singleton effects are parsed-not-applied (same posture as the 0x92/0x94 flags rows above) |
| status-set family: four cases storing fetched values into entity status words +368/+372, one bit-31 latch with a helper-fail halt; out-of-range subs spin on the zero-register add-by-register tail | `OP_STATUSSET` arm + `sub_size` row; widths pinned by `statusset_widths_answer_from_the_handlers_own_advance_sites` (this pass) | differs by named decision — widths equal case-for-case this pass and the unknown-sub spin matches kuluu's halt; the status-word stores (+368/+372 — the door-status slot among them) and latch are parsed-not-applied |
| item-info family: eight-word record builds behind two window openers, seven-word detail staging at fourteen, a widget fill gated on existence, sub 0x11's window-hold player wait, out-of-range subs landing on the advance-four default | `OP_ITEMINFO` arm + `sub_size` rows incl. the `Some(4)` default twin; refuses via `is_input_wait` (`input_wait_polls_stop_rather_than_advancing_unanswered`) and `unsupported_query_workflows_do_not_send_or_fabricate_results` (this pass) | differs by named decision — widths (incl. 0x20's default four) verified against the byte-map table this pass; window contents refused, whose on-screen role is Not established item 11 |
| recipe send/poll and ranking-board families: packet-88 builds with command words, pending-byte arms, poll-spins, yields on every path, widths two/four/eight/ten/twelve/fourteen/eighteen | no arm — refused; `sub_size` rows carry every width; refusal pinned by `unsupported_query_workflows_do_not_send_or_fabricate_results` | differs by named decision — retail parks until the server answers; kuluu halts instead of sending without a reply path (census: 40 sites / 10 zones and 28 / 6, none on a shipped cutscene's main line) |
| magic pair 0x73/0xC4 via shared helper: two actor lookups, scheduler-slot equality gate, both-sides flags-word bit-9 gate, eighteen-way visual-vtable handoff with a four-char entry literal; every exit — gates included — advances exactly eleven stream bytes; unknown 0xC4 subs fall out without advancing | no arm — generic width skip at the declared size eleven (`OP_MAGICSCHEDULOR` / `OP_MAGIC_TWIN` meta rows) | matches on control flow (the shared advance tail makes kuluu's size-skip faithful whether or not retail's gates passed); the visual scheduler handoffs un-run, entry names unread below the vtable |
| string-variable family: paired narrow/wide dispatch (subs 0x0A–0x10 reuse cases 0x00–0x06's bodies), width self-selecting from the word fetched at +8 (narrow eight/six, wide ten), registry indices below/at-or-above 64 splitting work slots from re-based external string blocks, sixteen-byte slot stores at stride two, compare cases branching to authored labels on mismatch | `OP_STRINGOPS` arm + `sub_size` rows; widths pinned for the family in `sub_width_opcodes_advance_by_their_case_width` (0x08→twenty-three); sweep refusal-free posture above | differs by named decision — every encoded width equals the handler this pass and no authored case refuses under the VM sweep; label-jump-on-mismatch is not modelled (kuluu falls through), registry/external-block string stores are parsed-not-applied |
| fourth-flags-word force-setters: bit 26/11/12 of [entry+300] := low bit of the inline operand (byte at +1; for the width-three op the fetched word's low bit — same byte), subject always the current event entity read from stream state, null-slot write skip advances anyway | arms `OP_ENTITY_WORD12C_SET_BIT26`/`_BIT11`/`_BIT12` → `EventCue::ActorRenderFlag` in the new `ENTITY_WORD_12C_BASE` space (`ffxi-event/src/cue.rs`); test `word12c_force_setters_pin_one_bit_each_to_the_low_bit` (pins bit numbers, both low-bit values and all three widths) | matches for parse/width/value law; host effect missing by decision — no source names these bits either (same posture as the +288/+296 flag rows) |
| item-card switch: operand zero closes/resets the menu handler object through its staged-record pair, non-zero opens that card with the operand as item id via the mode-0 opener both this record (Not established #11) and 2026-10-15-menu-docking-and-flags.md already name | no arm — generic width-three skip | matches on control flow; differs by named decision — kuluu has no event-driven item-card surface, the law above is its spec-away arming brief |
| pair dispatchers: 0xAD resolves two actors (staleness recheck against their table tokens, both-side eligibility bits) and performs one of ten module-literal-dispatched pair actions on the first actor's handle object; 0xAE weather/colour/link family writes entity +472 and the +1944 link slot or clears it, case widths six/eight/ten equal kuluu's parsed rows | AE armed width-only this pass (`OP_PAIRDISPATCH_AE` advance arm; unknown subs halt as retail spins — a behaviour change from the old fixed-ten fallback); AD stays un-armed, its flat twelve already equals the generic skip | widths pinned in `sub_width_opcodes_advance_by_their_case_width` (six/eight/ten) and the spin-halt in `sub_width_opcodes_stop_rather_than_falling_back_to_the_fixed_size`; matches on all advance paths including 0xAD's gate-failure twelve; handle-interface semantics, weather callee and link readers parsed-not-applied — blocker named in Not established #12 |
| small handlers audited this pass: 0x95/0x96 entity-refresh method pair on the current subject (word-fed and bare forms, widths three and one); 0x97 window-singleton dword pair (+312/+316) from words at +1/+3; 0xA8 widget staging plus map-window close behind sub byte one/two with a singleton null gate; 0xD3 zero-sub-only actor probe whose non-zero selectors spin; 0xC2 two query-to-work-slot forms (four/six) with a jump-set-two default; 0xC3 five-dword staged-record copy from helper block three, out-of-range index spins | D3 armed (`OP_STAGED_RECORD_PROBE_D3`) with both pins above; the six others un-armed — every effect is a client global or handle method below kuluu's cue boundary, and 0xC2/0x97-style stores have no consumer here | matches for every width and spin law kuluu can express (D3's halt pin added); parsed-not-applied list in the section above; zero authored uses across the corpus for all seven |
| census tail two latent over-skips closed: BF registry lookup advances on exactly three selectors (eight / ten / ten) and spins elsewhere including above its own ceiling; B1 mode-bits writer advances four on selectors zero-two and spins beyond | `OP_REGISTRY_LOOKUP_BF` + `opcode_meta.rs` sub_size row; `OP_MODE_BITS_WORK_WRITER_B1`; pins added to `sub_width_opcodes_stop_rather_than_falling_back_to_the_fixed_size` (BF interior selector, BF ceiling, B1 selector three) and `..._advance_by_their_case_width` (8/10/10/4) | matches for every advance path; the registry/mode values themselves stay below kuluu's cue boundary (producers unnamed — census tail two section) |
| fourth-flags-word family extended: unconditional bit-zero set on the event entity; resolved-actor bit-three force from byte@+1 | `OP_ENTITY_WORD12C_SET_BIT0`, `OP_ENTITY_WORD12C_SET_BIT3_RESOLVED` in `ENTITY_WORD_12C_BASE` space; tests `entity_word12c_bit0_op_is_an_unconditional_set`, `entity_word12c_bit3_resolved_form_names_its_actor_and_low_bit` | matches for parse/width/value; host effect unnamed as usual |
| QUERYWAIT2 poll: spin-while-unanswered, advance one answered, cancel stores the sentinel-kept 255 rather than ending | `OP_QUERYWAIT2` arm over `select_choice`'s zero-based store + `CHOICE_CANCELLED_QUERYWAIT2` remap; tests `op_7f_querywait2_yields_then_resumes`, `op_7f_querywait2_does_not_cancel_on_a_cancelled_choice`, `op_7f_querywait2_with_no_menu_open_skips` | matches — ledger-audited close this pass: retail's keep-255-else-decrement law against one-based-authored ids is kuluu's zero-based store plus the 254→255 remap, observably identical to downstream `if`s |
| mover-bridge writer: five selector bodies advancing uniformly ten / eight×four on every path (gates included), selectors above four spinning with no exec write | armed width-only this pass (`OP_MOVER_BRIDGE_B7`); pins in `sub_width_opcodes_advance_by_their_case_width` (ten and two of the eights) and `sub_width_opcodes_stop_rather_than_falling_back_to_the_fixed_size` (selector five) — before it the fixed-ten fallback over-skipped every spin case; mover-side work-slot stores parsed-not-applied (Not established #13) | matches for every advance path |
| staged-block writer: raw four-field copy at six, one scaled field per case at eight, all three chained at twelve, above-selector and gate failures stepping the entry-loaded six on the shared tail | armed width-only this pass (`OP_EVENT_DIR_WRITER_D8`); five pins in `sub_width_opcodes_advance_by_their_case_width` quoted from the handler's own advance-register loads (six/eight/twelve/above-ceiling-six); meta default row unchanged; gate-fail-inside-live-case divergence named above | matches for every selector path kuluu can observe — including 124 linear-walk sites whose width-eight records (selector samples one/two/three) the old fixed-twelve fallback over-skipped by four |
| queue audits 0x04/0x91/0x83/0x85/0x9C un-armed (width-faithful skips: three/three/one widths, `inc` tails noted), 0xA2 the third magic-pair handler whose success path parks with no advance while its fifteen-wide failures match kuluu's generic skip | no arms; laws recorded above as the spec-away arming brief (same posture as the 0x73/0xC4 pair rows) | matches on all failure paths; conditional park unmodelled at thirteen linear-walk sites (seven zones) — divergence only where the overlap predicate answers "separate" |
| work-slot float triple: two selector bodies advance eight (tail @100bba60), everything else reaches the pops-only exit where retail spins | armed width-only this pass (`OP_WORK_SLOT_TRIPLE_READ_AF` + `sub_size` row {zero/one → eight}; unknown subs halt) — pins in `work_slot_triple_advances_eight_on_both_selectors` and `sub_width_opcodes_stop_rather_than_falling_back_to_the_fixed_size` (selector two); the triple source object's role is unnamed, parsed-not-applied | matches for every advance path; posture-neutral (zero authored uses), counts unchanged |
| staged-slot loader: runs only on a resolved operand inside its eight-entry table — in range advance seven (@100bc04b), negative or above it no advance at all (@100bc00c/@100bc011, retail spins) | armed control-flow-only this pass (`OP_STAGED_SLOT_LOADER_C3`) reading the operand through `getworkofs` because every authored site is reference-window-bound; five-dword staging table and fetched blocks below kuluu's cue boundary | matches for both paths — in-range width and out-of-range halt pinned by `staged_slot_loader_halts_outside_its_index_table`; the first arming attempt (raw word read) refused 17 corpus events, caught by re-measuring the sweep, not by reading code |
| ledger reason-close pass: every non-MATCH row carries its recorded verdict plus reason; audit reasons print in the notes column, machine facts beneath; twenty handlers read to their last instructions across this pass and the two before it | AUDITED dict entries staged through `entries_a/b/c/d` appliers; classifier notes-placement change in `scripts/opcode-parity-ledger.py`; census MATCH 184 / NO-ARM 20 / WIDTH-ONLY 15, zero thin cells (checker asserts) | closes the bookkeeping the grind exists for; it then produced two real arms (the two rows above) and one retraction — 0xC2's claimed default-selector over-skip does not exist, kuluu's generic skip already carries that selector |
| use-count semantics note (this pass): two measures coexist in this record — `zz-opcode-gap` **linear-walk encounters** along thread byte streams, and VM-sweep **executed** paths; rows earlier here saying an op has "zero uses" predate this distinction (e.g. the small-handler seven show 46–267 linear-walk sites while raising no sweep refusals — never executed on driven paths). Quote the measure, not just a count | `ffxi-event/examples/zz-opcode-gap.rs` (encounters) vs `examples/zz-event-sweep.rs` posture (executed) | clarification only |
| the two "handler not read" WIDTH-ONLY rows closed: 0x7A six cases under a ceiling of five (child attach/detach, predicate parks answering two, case widths six/seven/six/two/eight/six); 0xB5 name-patch with its pending-spin non-zero subs | existing arms unchanged; `sub_size` rows equal every advance site for site; five new pins in the sub-width tests (`..._advance_by_their_case_width`: exec-context subs two/three/four = six/two/eight; `..._stop_rather_than_falling_back_to_the_fixed_size`: exec-context ceiling and name-patch selector one) | matches — closes the last handlers the ledger carried unread; effects parsed-not-applied, open halves as Not established #15 |
| a fixed client byte is zeroed by opcode 30 and set to one by the engine elsewhere; kuluu now performs the store instead of skipping it | `OP_CLIENT_BYTE_10488100_ZERO` arm → `EventClientGlobals::byte_10488100`; pin in `fixed_client_global_stores_write_their_cells_and_nothing_else` (advance one, no cue) | matches — the store happens; kuluu has no consumer for the value yet, and that is stated rather than used as a reason to omit it |
| opcode 0xD9 stores "operand byte equals zero" into a fixed client byte cell; opcode 0x9E stores the same predicate as a dword (exactly one or zero) | `OP_CLIENT_BYTE_10463924_OPERAND_IS_ZERO` / `OP_CLIENT_DWORD_104898FC_OPERAND_IS_ZERO` arms → `EventClientGlobals::byte_10463924` / ::`dword_104898fc`; same pin, operands 0/1/255 with advance two each | matches — value and width pinned per operand |
| opcode 0x85's operandless call posts a named menu window through the UI singleton (three possible windows, chosen by the run-state word test and the flag-window test) then advances one | no arm: `zz-opcode-gap` lists it, ledger NO-ARM; kuluu's width-one skip drops the posting entirely | differs — this is a real UI effect with no kuluu counterpart. Arming brief is in the section above and Provenance (window names, both gates, all sites); blocked on kuluu having no event for "open this named menu window" |
| the coverage probe's "no arm" verdicts: six parsing faults in that example (value read as the first hex run, only bare `const` visibility, vm.rs-only constant pool, shared/multi-line dispatch patterns, its own `#[cfg(test)]` constants counted as production, one alias per opcode) reported armed opcodes as gaps — measured 217 → 33 → 25 → **22** true gap rows, so gap-based prioritisation was partly noise and three of its top-five "gaps" were arms | `ffxi-event/examples/zz-opcode-gap.rs` (`consts_of`, `without_tests`, alias-per-opcode map + `uses_beyond_declaration`) — fixed this pass; corpus walk and operand sampling untouched. Cross-checked against `scripts/opcode-parity-ledger.py`: 16 of the 22 are ledger NO-ARM, 6 are audited-MATCH effectless/stub ops whose skip is retail's response, and 4 further ledger NO-ARM rows stay out because kuluu names them in a refusal arm | differs, tool only — no shipped behaviour changed (corpus encounters, sweep posture and all 254 tests identical across every fix); the honest gap list is pasted in full in Provenance so future claims quote post-fix data |

## Not established

1. 0x6A's second argument to the four mixer change-arms (second work operand): fade frames per its
   position and reuse across all arms, but unconfirmed without reading the mixer functions.
2. What retail's two flags-word bits (0x92/0x94) and visual-half byte (0x81) do on screen: readers
   of the flags dword cannot be separated from its hundreds of unrelated uses statically; no reader of
   the visual-half byte exists in this image besides possible aliasing. A retail-observe pass naming
   them is the only way forward; kuluu stores and publishes meanwhile.
3. The composite order and tick behaviour of the colour-drive task behind the shared scheduler-stage
   vtable (same blocker as today's scheduler record, now with the same-task-class evidence added).
4. ~~Which window-animation state 0x75's hold tests, and what its skip-over-two case pairs with~~
   **answered the same session** — the hold is the window singleton's any-open count (never true for
   kuluu), and case 2 carries the previous instruction's last WORD as a packet-242 payload; see the
   map-window section above. What remains open there are only the two named missing counterparts
   themselves, not the law.
5. Whether any authored script relies on MAPLOAD's counters being shared across concurrent event
   threads (kuluu keeps them per VM); none identified in corpus review this pass.
6. What the actor-data switch (0x59) staging writes *do*: no opcode reads the +220/+224 custom
   slots back, and the found readers sit on mover-mirror-family objects (`f_100ac503`, `f_100c4820`)
   whose consumption was not opened; likewise unread are the visual-object vtable internals behind
   slot +672 (busy predicate) and +556 (value sink), and which client code populates
   `[entry+452]`. A retail-observe pass naming them is the way forward; kuluu parses and advances
   meanwhile.
7. The scheduler-helper family (`f_100b5510` / `f_100b5210`, feeding 0x5F cases ≥2 and 0x66):
   narrowed by the 2026-10-29 revision — case layouts, widths (including the bump on the two
   operand-carrying cases), both park gates, the scheduler's per-slot pending-load byte, and the
   anchor arithmetic are established. Still open: which stream word actually feeds the bucket chain
   (its repeated re-fetches read a literal operand index through that phase), what the id
   transform's mid-range table branch leaves behind for later fetches to pick up, and which motion
   packages the anchored buckets name. Also still open: whether any client code reads back the
   vtable field the 0x7B neighbour writes to its 16-bit all-bits value (both sites in Provenance).
8. What 0x60's singleton handoff (the global-pointer-named singleton and its dispatch entry 24
   (addressing in Provenance), called with the resolved actor) performs; entry internals unread.
9. Which client-global slot receives 0xAB sub 7's fetched value, and who reads it: the store site
   is quoted in Provenance but the slot's readers were not separated from its unrelated uses. The
   visual role of the six render-flag bits this pass mapped (second/third-flags words +288/+296)
   is likewise unnamed — same blocker as item 2 for the earlier pair.
10. What the ranking-board singleton stores between 0xB3's writes and what redraws it (its
    holding global is quoted in Provenance; board object layout unread); which authored flows reach
    the 40 recipe-send sites at all (their events were seen by linear walks — follow-up jump paths
    not traced).
11. The on-screen identity of 0xCC's two window families (the examine-style opener behind
    `f_1013e520` and the detail opener behind the item-record staging) past their record
    mechanics: which retail UI each corresponds to was not confirmed beyond the field layouts.
    **Sharpened this pass:** 0x93 opens exactly that examine-style card by item id (and closes it via
    the manager reset), so if a kuluu item-card surface ever exists, `f_1013e520` and the window layout
    its Provenance entries anchor is the thing to copy.
12. What lives below the pair dispatchers' call sites (this pass): the ten handle-object dispatch
    entries (+708..+748 range on `f_100bb340`'s target), 0xAE case zero's singleton callee
    (`f_10094040`), and every reader of the actor link slot at +1944 that both 0xAD-style pair cases
    and 0xAE write (store sites under Provenance). All three unread; none are reachable from kuluu's
    cue surface, so the width-faithful skip stands until a retail-observe pass names what each does.
13. Who reads back the mover-side stores 0xB7 case zero/case one make into a mover block's own
    work-slot pair (and its name-pair copy): no opcode was found addressing those mover-block slots,
    and the blocks' field layout past that pair was not opened (store sites under Provenance).
14. What consumes 0xB7 selector four's eight-entry global word array, and what the current event
    entity slot +364 (float, written by 0x91 at ×ten) does with its value: both bases are quoted
    under Provenance; their readers were not separated from unrelated uses of those globals.
15. Who writes 0x7A case three's saved execution trio (+0/+4/+604 on the thread block, restored
    into +8/+2/+608): nothing inside `f_100b3a50` touches them, so an interpreter-side switch (or
    a mover-thread construction path outside this handler) does; its sites were not sought past
    this handler's boundary. Same open question for what the mover method handed the 0x7A case
    zero/one helper (`f_100b3830`) then performs.

## Provenance

- Dispatch reading (2026-10-29), all in `funcs_012.c`: interpreter loop `f_100bccd0` at instr 100bcce3
  (`mov cx, word ptr [esi + 0x256]`), 100bccea (`movzx dx, byte ptr [ecx + eax]`), 100bccf2 (`call f_100bc280`), then the
  park-flag read `mov al, byte ptr [esi + 0x25a]`. Dispatcher `f_100bc280`: `cmp eax, 0xda` (100bc285),
  `ja 0x100bc957` (100bc28a), `jmp dword ptr [eax*4 + 0x100bc960]` (100bc290). Slot values were read as raw bytes out of
  `research/xi-on-anything/generated/FFXiMain.unpacked.dll`; stub→handler pairs come from the same build's generated C
  (`L_100bc297 .. L_100bc95f`), names from `table.c`. Extracted by `scripts/opcode-dispatch-table.py`, which refuses any
  switch table that does not resolve the four pinned opcodes (0x1F/0x63/0x6E/0x99) to their record-cited handlers.

Generated C: `research/xi-on-anything/generated/all` (`funcs_012.c`, `funcs_013.c`, `funcs_005.c`),
image reads via pefile from `research/xi-on-anything/generated/FFXiMain.unpacked.dll`. The opcode
dispatch jump table for the event interpreter is at image VA 0x100BC960 (slot = opcode byte); every
handler cited below is that slot's entry, verified against anchors 0x63→f_100b7550 and 0x4C→f_100b6950.

- Volume: `f_100af4b0` = GetWorkofs (word at code+ip+n routed via `f_100af4c0`). 0x69 handler
  `f_100b7a60`: value dword 0x3f800000-or-0 from raw byte `[codebase + ip + 1]`; mask = low byte of
  GetWorkofs(2); arms bit1→`f_10036020`, bit2→`f_10036070`, bit4→`f_10035f80`, bit8→`f_10035f00`;
  bit 0x10 → `f_10311c6c(float)` written to dword global 0x10351710. Width: add ip, 4. 0x6A handler
  `f_100b7af0`: mask = GetWorkofs(5) low byte; second arg from GetWorkofs(3); target float = int
  (GetWorkofs(1)) × rdf32 [0x1032a22c] (= exactly 0.001f); arms bit1→`f_10035d40`, bit2→`f_10035d90`,
  bit4→`f_10035de0`, bit8→`f_10035bc0`. Width: add ip, 7.
- Doors: first pair `f_100b6950`/`f_100b6a00` write 8/9 at `[actor+0x174]`, gate test byte
  `[actor+0x120] & 4`; entity word read from `[exec+2]` (the block's own event entity — same as
  kuluu's existing door target). Second pair `f_100b6980`/`f_100b6a30` write 0x2d/0x2e, identical
  otherwise. Widths 1 each (`inc word [exec+0x256]`).
- Flags: eligibility pair `f_100afac0` (assembles the LE dword lookup operand) + `f_100afb00`
  (entity-table fetch, presence via `f_1008b380`; al==1 eligible). Bit toggles `f_100b9ba0`: bl =
  `[codebase+ip+1]`, shl 0x10, xor against dword `[actor+0x12c]`, and 0x10000 → flip bit only when
  differing; `f_100b9cb0` identical with shl 0x11 / mask 0x20000. Widths: add ip, 6 for both (opcode,
  operand byte at +1, dword lookup at +2). `f_100b8fb0`: gate `[actor+0x120]` shr 9 bit-then-1; visual
  pointer `[actor+0xa0]`; writes 1/0 to `[visual+0x8B4]` from inline byte at +1 non-zero; add ip, 6.
  Writers of `[visual+0x8B4]`: `f_100b8fb0`, `f_100ae3f0`; also touched in `f_100c5d60`, `f_100c63d0`,
  `f_101d7c10`, `f_101d8140`. No readers located.
- Map: shared window helper `f_100b2570(mode, select, value, a4, a5)`. 0x8D handler `f_100b9330` =
  (mode 1, GetWorkofs(1), GetWorkofs(3), 0, 0); add ip, 5. Marker plain `f_100b9490` (kuluu's shipped
  MAP_MARKER): words at +1/+3/+5/+7 via GetWorkofs(n) for n=1..7, name dwords copied from
  `[codebase+ip+9]`, per-byte 0x5f→0x20 rewrite loop over the sixteen bytes, map singleton
  global 0x10663888; setter trio `f_101f8d50` (position x/z ×0.001 via [0x1032a22c]), `f_101f8de0`,
  `f_101f8e30` (name); add ip, 0x19. Marker with selector `f_100b9600`: words via GetWorkofs for
  n=1..9, name at `[codebase+ip+0xb]`, same rewrite loop; window-state global word [0x104896e6]: non-
  zero → helper mode 1 + `f_101f8de0` call, zero → helper mode 4 and no icon setter; singleton null →
  skip everything; both exits add ip, 0x1b.
- Map sub-op 0x75 `f_100b8660`. Gate: `[0x1065cb04]`→`f_1018ba60`, which tail-calls singleton
  0x10631f24's `f_10178780` — returns `[obj+0x44] != 0`, i.e. an any-open window count; true →
  yield byte at `[exec+0x25a]` (@100b8672), no advance, ret. Selector = data[ip+1] read @100b8690
  with ip from the word [exec+0x256]. Sub 0 → @100b86eb: fetch via `f_100af4b0(2)`; resolved value →
  `[0x10631f24]`→`f_1018baa0(v)` → `f_10178900(id,1)`: stores id to `[obj+0x48]`, present chain
  (`f_101779b0` slot lookup, window lists on singleton 0x1047d128 via `f_100731b0`/`f_10072fd0`),
  one branch calls `f_100fc680([obj+0x48],1)`; then @100b8700 `add word [exec+0x256], 4`
  unconditionally. Sub 1 → @100b86df: `add eax,2; mov word [exec+0x256]` only — no operand fetch,
  no call. Sub 2 → @100b86a2: stores `[exec+0x256] := ip−6`; `f_100af4b0(2)` then reads the WORD at
  stored+2 = original ip−4 (previous instruction's last word); `f_100fc680(2, v)` @100fc680: tests
  singleton [0x104dfd98], allocates c2s packet id 0xf2 via `f_100fc3c0`, writes WORDS `[pkt+4]=2`,
  `[pkt+6]=v`, sends through `f_100ddd20`; accepted → @100b86c4 `add word [stored], 8` (net ip+2),
  refused → @100b86ce add 6 (restores original ip) plus yield byte `[exec+0x25a]=1` — spin.
  Unknown selector falls to @100b8708 pop/ret, exec untouched. Fetch convention settled from
  `f_100afa90`: reads little-endian WORD at `[data + ip + arg]`/+1 and never writes [exec+0x256],
  so the explicit adds @100b8700/@100b86df-@100b86c4/@100b86ce are every advance in the handler.
- Turn-wait 0x70 `f_100b7dd0`: entity word @+1 → table slot; null jumps to the width tail (@100b7df
  a, `inc word [exec+0x256]`); `[entry+0x12c]` shr-1 bit-test set → yield byte @100b7df1, no advance;
  clear → `call f_1008af20` then inc. `f_1008af20`: gate `test ah,2` on `[entry+0x120]` (bit 9);
  visual = `[entry+0xa0]`, two calls to visual vtable **+0x29c** with push-params ('@tl0'=0x306c7440,
  '@tr0'=0x30727440) and the visual pointer.
- Music-read 0x9A `f_100b6df0`: index = signed byte [0x10482f1f]; compare word @0x10485fe4+idx*2 vs
  tracker word [0x10482f1c]; mismatch → yield, no advance (@100b6e24). Match → `call f_10037320`
  ([0x10463944] null → al=0; else sets byte [0x10463909]=1 and returns al = ([0x10463944] !=
  [0x10463940])); yield byte set at @100b6e12 on every gates-clear pass; al==0 → `inc word
  [exec+0x256]`.
- Chocobo 0x7E `f_100b8b80`: guard `cmp eax,8; ja @100b8bc5` (`mov ebp,[esp+0x10]` — stack garbage).
  Width table @100b8e44 dumped from image bytes via pefile: subs 0/1/2/4/5/8→@100b8ba9 (`ebp=6`),
  3→@100b8bb0 (0x10), 6→@100b8bb7 (0x12), 7→@100b8bbe (8). Gate trio: `f_100afac0(2)`+`f_100afb00`
  (`cmp al,1`), slot `[edx*4+0x10480af0]` non-null (`cmp eax,ebx`), `shr dword [entry+0x120],9;
  test al,1`; all fails land on shared tail @100b8e34 `add word [exec+0x256],bp`. Second table
  @100b8e68: sub 0/5→@100b8c48 (gate `[entry+0x120]&4` → store `[entry+0x174]=ebx`=0); sub
  1→@100b8c6c (store 5; `f_10097a20(1)`); sub 8→@100b8c99 (store 0 under gate; `[entry+0x1db]=bl`=0);
  sub 7→@100b8cc1 (store 0x55 under gate; fetch(+6), `inc`, `[entry+0x1db]=al`); sub
  2→@100b8cf7 (`shr [entry+0x120],0xa` + visual `[entry+0xa4]` non-null → vtable **+0x1e4**
  predicate; true → width tail, false → bare ret @100b8d29 no advance = park); sub
  3→@100b8d31 (store 5 under gate; `f_10097a20(2)`; fetch(+6) then `add eax,0x20`, fetch +8/+A/+C/+E
  into a stack block, two zeroed words, `call f_1009ac10(visual, block)` at @100b8e2f); sub
  6→@100b8da4 (same body also fetching +0x10).
- Friend pair 0x87 `f_100b9140` / 0x88 `f_100b9220`: sub switch by compare chain; send bodies
  `call f_100fc3c0(0,0,0x1b)` (packet id 27), `[pkt+4] ← 0/2` (@100b91e1/@100b9187 for 87) or
  `← 1/3` (@100b92ca/@100b9267 for 88), finalize `call f_100fc4d0(pkt,0,0x1c)`, in-flight byte
  global [0x1048557a] = 1, `add word [exec+0x256],2` + yield. Alloc null → pending-only tail
  (@100b9207/@100b92ed), no advance (retry). Poll sub 2: latch byte read into al, yield always set,
  `test al,al; jne` non-advance when set — clear advances two (@100b91bd/@100b92a1).
- Movers. CodeSMOVE `f_100b5630`: guard sub≤8; poll body @100b565a — slot triple
  `[slot*32 + exec_state + 0x28/0x2c/0x30]` minus entity floats `+0x140/0x144/0x148`,
  `fpatan → [entity+0x154]`; budget gate `cmp word[slot*32+exec_state+0x38],0; jle @100b5766`
  (skip all integration); arrival `f_10014cf0() × [entity+0x168] × const@0x1032b160 ≥ dist` → snap
  triple (@100b5712) then @100b5766 `add word[exec+0x256],2`; else integrate each axis
  unit-vector×step and jmp @100b576e skipping the add; every pass: countdown write-back
  (round(staged − f_10014cf0()) back into the slot word, @100b577a..@100b57a7),
  `[entry+0x124] |= 0x20000` (@100b57be), yield byte. Stage @100b58ab: `f_1008af20`, three scaled
  fetches, then raw resolved word → `mov word [slot*32+exec_state+0x38], ax`; `add ip,0xa`.
  Tail @100b5954 (every path): skip when `[entry+0x128]>>0xe | [entry+0x120]>>0x15 | >>0x14` bit0
  set; else singleton 0x10631f24 `f_10181290(…, literal 0x42480000=5000.0)` (entity id equal to
  control word [0x10485f7a]) or `f_10181300` otherwise, out-param float replaces `[entity+0x144]`
  when it returns 1. CodeMOVE2 `f_100b31f0`: same pair minus budget — stage @100b3428 (three scaled
  fetches, `add ip,8`); poll integrate uses weight const [0x1032961c]/dist × per-axis component with
  jmp @100b33fd skipping the add; snap path copies staged triple and adds two; both paths set
  `[entry+0x124] |= 0x20000` + yield byte; unknown sub advances nothing (@100b34b7 pop).
- MAPLOAD: handlers `f_100b5a90`/`f_100b5b70`; counter dword [0x10489910] (armed value 0x12c),
  pending-reload dword [0x10489918], hold flag written at `[exec+0x25a]` and global byte
  [0x104898f9]; idle pass loops the entity array 0x10480af0..0x10482ef0 through `f_10092900`; armed
  pass: pending≠0 → close via `[0x1065cb04]`+`f_1018b620` (MAPLOAD only), else teardown chain
  `f_100b5c30` (globals + `f_10014cb0`/`f_1018b130`/`f_1001e470`) then open `f_1018b3a0(
  GetWorkofs(1))`; terminal add ip, 3. MUSICREADWAIT handler `f_100b6df0`: signed byte index
  [0x10482f1f] into word table @0x10485fe4 compared against word [0x10482f1c]; on match call
  `f_10037320`, advance (add ip, 1) only when it returns false; either mismatch or true re-park via
  `[exec+0x25a] = 1`.
- Map sub-op family handler `f_100b8660`: hold on `[0x1065cb04]`+`f_1018ba60`; inline byte at +1:
  case 0 → GetWorkofs(2) then two-arg call `f_100fc680(2, w)` with per-outcome advances (+8 or hold);
  case 1 (jump target when byte==1 path) closes via `[0x1065cb04]`+`f_1018baa0(GetWorkofs(2))`, add
  ip, 4; case 2 → add ip by two (rewrites `[exec+0x256] += 2`); byte==3 falls through with no
  advance. `opcode_meta.rs` records base width 4 for the size-skip kuluu keeps meanwhile.
- Scheduler stage dispatch: task factory function region `funcs_005.c`, bounds-check + computed jump
  at VA 0x10057fc9..0x10057fd8 (`edx = type - 2; cmp edx, 0xc1; ja → unknown label L_1005ac96`; cells
  read from dword table at image VA 0x1005dc1c). Anchors: cell for raw 0x2D = 0x1005C8BA (the
  orientation-hold handler cited by today's scheduler record), cell for raw 0x62 = TurnToward. Raw
  0x51 → label at 0x10058F8F and raw 0x72 → 0x10059088: both allocate 0x4c via `f_10028fd0`, set the
  same instance vtable dword 0x1032B8BC, store `[task+0x48] =` their stage-type byte (0x51/0x72), copy
  payload bytes [stage+8]/[stage+9]/[stage+a] → [task+0x46]/[task+0x45]/[task+0x44], and compute the
  countdown from signed word `[stage+6]` × float `[factory+0x9c]` through `f_10311c6c`; both set byte
  global [0x1047d13c] = 1. Census counts/words for the two kinds (8 and 6 sites, three words each,
  triples per today's scheduler record) reproduced by `cargo run -q --release --example dat-routine-
  stages` over carrier window 30904..30912; scratchpad digests handlers-batch3.txt / opcode-gap.txt.
- Store-family pass (2026-10-29 night), generated C `research/xi-on-anything/generated/all`:
  random stores `f_100b04c0` (draw + setworkofs(@+1), add ip, 3) and `f_100b04e0` (divisor =
  GetWorkofs(+3)+1 via `inc esi`; `je` branches around the draw to a zero store; else `idiv`, remainder
  stored; add ip, 5). Draw helper `f_103149e0`: state `[obj+0x14]`, `imul ecx,ecx,0x343fd; add
  ecx,0x269ec3`, output `(state >> 16) & 0x7fff`; kuluu mirrors it verbatim — first draws from seed 1:
  41, 18467.
- Trig pair `f_100b0600` (sin) / `f_100b0660` (cos): angle fetched at operand+3, amplitude at +5 after
  it; angle scaled by dword `[0x1032d694]` = bits `0x3AC90E56`; sin `fchs`es the product before the
  conversion, cos does not; shared float-to-int `f_10311c6c` = control word to nearest + `fistp qword`
  (round-half-even, low dword read back); store setworkofs(@+1), add ip, 7.
- Heading variant `f_100b06c0`: operand@+5 fetched first (FPATAN x-axis value), @+3 integer-negated
  (`neg eax` at 100b06e2) as y; × dword `[0x1032d3d8]` = `0x4580_0000` (4096.0f), then ×
  `[0x1032b170]` = `0x3EA2FABE`; store, add ip, 7. Adjacent `[0x1032b160]` = 0.0166..f (1/60) seen in
  move handlers — recorded this pass, not used.
- Remainder `f_100b61a0`: operands +3 dividend / +5 divisor; either zero → the `xor edx,edx` guard path
  stores 0 (`idiv`, EDX stored); add ip, 7.
- END twins: dispatch slots 0xCA/0xCB hold stub target `f_100afc80`, identical to slot 0x00. That
  handler resets `[event + slot*0x20]` fields (+0x24=0xff, word +0x26=0, dword +0x3c = -1.0f
  `0xBF800000`, dword+0x00=0, byte +0x3a=0; slot index = word `[event+0x258]`) and writes the yield
  byte `[event+0x25A]`; it never touches exec.
- Deferred-op globals named this pass: frame-factor add (slot 0x57) `f_100b0350` calls clamp helper
  `f_10014cf0` = min(dword `[global+0x28]`, dword `[0x1032961c]` = 1.0f); slot 0xD9 writes byte
  `[0x10463924] := (inline byte == 0)` — other static uses of that byte sit in `funcs_003.c` around
  RVAs 100356d8/10038364; slot 0x30 = `mov byte [0x10488100], 0; inc word [event+0x256]`.
- Work distance `f_100b75c0`: GetWorkofs(7)/(3) → edi = x⁺−x⁻ (the second sub precedes the third
  fetch); GetWorkofs(9)/(5) → ebx pair; `imul` each with itself, `fild`/`fiadd`, `fsqrt`,
  `call f_10311c6c`, store via `push eax; push 1; call f_100af310` (setworkofs +1); add ip, 0xb.
- Actor distance `f_100b7860`: lookups `f_100afac0`(push 3) and (push 7), each through
  `f_100afb00`/entry-null gates; raw triples `[entry+4]/[+8]/[+C]`, interpolated branch via
  `[entry+0x120] shr 7` → `[[entry+0xd4]+0x260]` fields +0x140/+0x144/+0x148 (the two actors load
  the three axes in mirrored orders — squared-sum makes slot order irrelevant to the kuluu arm, and
  retail's exact pairing was not traced further); deltas `fsub`, products `fmul`, `fsqrt`, `fmul
  qword [0x1032a110]` (= exactly 1000.0), `call f_10311c6c`; every gate failure jumps to the same
  store with a pushed zero; add ip, 0xb.
- Actor-data switch `f_100b6a60`: selector byte `[codebase+ip+1]`, `cmp eax,8; ja` to a pops-only
  return (no advance = spin); jump table of nine entries at image VA 0x100B6DC0. Case 0 store site
  100b6ab7 (`[entry+0xdc]`), case 2 at 100b6b55 (`+0xe0`), lookup forms store at 100b6b1b/100b6bb9;
  case 4 float site: `fild/fmul dword [0x1032a378]`(= 0.1f)`/fstp [[entry+0xd4]+0x260 field +0x168]`
  at 100b6c29..100b6c34; case 5 xor-latch `shl ebx,0x15 / and 0x200000` against `[entry+0x120]`,
  value byte loaded from `[codebase+ip+6]`; case 6 requires `[entry+0x1c4]` + `[entry+0xa0]`, calls
  `dword [visual_vtable+0x2a0]`, `cmp al,1; jne advance-6` else writes yield byte 100b6d0d (no
  advance); cases 7/8 tail into `f_1008a790` = `tailcall [visual_vtable+0x22c](this=[entry+0xa0],
  value)` with its own null guard. Advance instructions per case: 4 (100b6abd/b5b/d56), bx=8
  (100b6b21/bbf/c3a/db1 — db1 is also the lookup-failure landing pad for cases 1/3/4/8), 7
  (100b6ca8), 6 (100b6d1a). Slot readers of `+0xdc`: `f_100ac503` copies `[x+0xdc]→[x+0x5cc]`, mover mirror
  reads it at 100c4820 beside the quad mirror; entity-init default `mov [entry+0xdc],0x64` and the
  same for +0xe0 (100ae656/100ae662). No dispatch-region reader of either slot exists.
- Heading read `f_100b2c80`: lookup pair `f_100afac0`(push **1**)/`f_100afb00`; refusal jumps
  straight to the width-add (no store). Entry `[edx*4+0x10480af0]`; null branch pushes zero into the
  shared store path at 100b2d08..100b2d0d (`push eax; push 5; call f_100af310` = setworkofs operand
  +5). Heading source: `[entry+0x120] shr 7 test bit` → set: `[[entry+0xd4]+0x260]+0x154`, clear:
  `[entry+0x18]`. Wrap helper `f_100dd9d0`: loop comparing against dword `[0x10331818]` = 6.283185f
  (`fnstsw/and 0x4100` → subtract K while ST>K), then against `[0x103295d8]` = 0.0f (`test ah,5;
  jp` ret, else `fadd K`). Scale `fmul [0x1032d3d8]` (4096.0f) then `fmul [0x1032d6b4]` = bits
  `0x3E22FABE` ≈ 1/2π; round via `f_10311c6c`; add ip, 7.
- Bit-array clear `f_100b02f0`: GetWorkofs(3)=bit index (call at 100b02f7), `sar edi,5`; bound from
  GetWorkofs(5) (100b0304); `jge` past the write when slot≥bound; prev via `push edi; push 1;
  call f_100af4c0` (getworkofs operand-1 word at slot, 100b0315); mask `mov edx,1; and ecx,0x1f; shl
  edx,cl; not edx; and eax,edx`; store via `push edi; push <val>; push 1; call f_100af330`
  (setworkofs, 100b0330); `add word [esi+0x256], 7` at 100b0335. Widths cross-check with slot 0x3C's
  handler.
- Entity flags-word twins: `f_100b35e0` — lookup pair `f_100afac0`(push 2)/`f_100afb00`, eligibility
  `cmp al,1`; entry `[edx*4+0x10480af0]` (low mask of the lookup), null skips; inline byte at
  `[codebase+ip+1]` → `shl ebx,0x13`; xor against dword `[eax+0x120]`, `and ebx,0x80000`, xor back,
  store; `add word [esi+0x256], 6` at 100b3647. `f_100b5a40` — entry from word `[ecx+2]` (the block's
  own event entity), same xor-latch with `shl ebx,0x15` against `[eax+0x120]`; width two per the slot.
- Thread-slot timer sites (`funcs_012.c`): slot float at `[event + word[event+0x258]*32 + 0x3c]`;
  −1.0f resets at 100afcb4 (END) and 100b079e; countdowns in `f_100b07f0` (0x6F: negative → store
  `0x41800000` = 16.0f at 100b0814; subtract clamp-helper `f_10014cf0` result via `fsubr`; yield
  byte always 100b0859; `add ip, …` skipped while ≥ 0 — the strict-below-zero expiry),
  `f_100b0870` (OP_WAIT: loads GetWorkofs(1) through `fild` into a negative slot at 100b08ac,
  same countdown, add ip, 3 on expiry), `f_100b0910` (0xB2 family, same law with case-dependent
  advance). Turn-wait `f_100b7dd0`: entry from word `[event+2]`; null → add ip, 1; `[entry+0x12c]
  shr 1 test dl,1` set → yield byte at 100b7df1 with no advance; else `call f_1008af20` then
  add ip, 1. `f_1008af20`: gated on `[entry+0x120]` bit 9 (`test ah,2`), tail-calls visual vtable
  +0x29c with pushes 0x306c7440/0x30727440 ("@tl0", "@tr0") on `[entry+0xa0]`.
- Ledger tooling: `scripts/opcode-parity-ledger.py` (verdict buckets, AUDITED overrides, thunk-helper
  recursion) over the slot table from `scripts/opcode-dispatch-table.py`. Fixed this session: constant
  names are now also collected from `opcode_meta.rs`, because the sub-byte-family constants
  (`OP_ENTITYSPEED` and friends) are defined there and their arms read "no constant, no arm" before;
  a name registered from meta counts as armed only when referenced by a dispatch body. Two rows
  surfaced by that fix were machine-MATCH on width/yield alone while no effect of any case is
  modelled — they hold a new **WIDTH-ONLY** verdict (0x7A, 0xB5) until their handlers are read.
  Full dump at
  `.agents/skills/retail-observe/references/data/opcode-parity-ledger.md`. The unpacked image is mapped
  by its own PE section headers (e_lfanew `0x138`) for every .rdata dword read.
- Latch-family and third-word batch (2026-10-29 late), every handler in `funcs_012.c` (verified with
  a per-symbol grep over `research/xi-on-anything/generated/all`), read via
  `scripts/opcode-body.py`; the dispatch-table dump (`scripts/opcode-dispatch-table.py`) supplies the
  op→handler pinning for the queue. 0x61 `f_100b74e0`: entity lookup at instr 100b74e2–100b74e6
  (`mov ax, word [ecx+2]`; `mov eax, dword [eax*4+0x10480af0]`), mask-OR write 100b750b–100b7511,
  step `add word [ecx+0x256], 2` at 100b7517. 0x60 `f_100b73f0`: selector read 100b7405, branch
  `cmp al,1/jbe` 100b7409–100b740b to the latch (xor-latch arithmetic 100b744d–100b7465 over
  `[ecx+0x124]`, advance-4 store at 100b746b), `cmp al,cl` (`cl`=2) at 100b740d to the singleton arm
  (`mov edi, dword [0x1065cb04]` 100b7412; `push ecx` (ecx=2 from 100b73f6) at 100b741e before
  `call 0x100afac0`; indirect `call dword [ebx+0x18]` 100b7429; advance-6 immediate at 100b742c),
  default arm `add word [esi+0x256], cx` (cx=2) at 100b747a. 0x5F's latch cases share that handler:
  jump-table dispatch `jmp dword [eax*4+0x100b73c4]` at 100b72e9, case bodies at 100b72f0 with the
  xor-latch arithmetic over `[eax+0x124]` (bit 29 — `shl ecx,0x1d`, mask `and ecx,0x20000000`) at
  100b7301–100b7317 and the advance-2 store at 100b731d. 0x74 `f_100b7490`: latch arithmetic
  100b74b3–100b74c6 (`shl eax,0x1f`, mask `and esi,0x7fffffff`), step at 100b74cd. 0x7C `f_100b8a80`:
  eligibility `push 2`/`call 0x100afac0`/`call 0x100afb00` at 100b8a90–100b8a9c, bit write
  `or ecx,0x20000` 100b8ae4 / `and ecx,0xfffdffff` 100b8afd, advance-6 on every path (100b8af0,
  100b8b09). 0x7B `f_100b8a00`: `[entry+0x12c]` clear `and dh,0xfc` 100b8a55–100b8a58 and
  `mov word [eax+0x148], 0xffff` 100b8a60 — read, not armed. Scheduler helpers: `f_100b5510`
  operand-token fetches (`push 1`) at 100b5522, id-pair compare `[eax+0x74]/[eax+0x78]` 100b5553–
  100b5563, flag gates bit9/bit7 of `[entry+0x120]` 100b5565–100b557b, load-through-`[eax+0xa0]`
  100b558e–100b559a; `f_100b5210` dual fetch (`push 3` at 100b5223, `push 7` at 100b524b),
  bucket compares against 0xc00/0x800/0x400/0x200 with literal adds 0x10323/0xe95b/0xdc19 at
  100b5357–100b53af; caller rewind `mov word [esi+0x256], di` at 100b73ba in 0x5F's handler
  `f_100b72c0`, jump table base at 100b72e9. Empty stub 0x6D `f_100b7850`: single instruction
  `add word [ecx+0x256], 7` at 100b7850.
- Audit band (2026-10-29 late), every handler verified to live in `funcs_012.c`, read whole-body
  via `scripts/opcode-body.py`. Yield trio: yield-byte-only store at 100afc70 (`mov byte [ecx+0x25a],1`);
  `inc exec` + yield at 100b0380–100b038e; MESWAIT state byte read from `[0x10488208]` at 100b2dd0,
  states branched 100b2ddb–100b2de3, speaker-slot clear via `[esi+0x260]+0x164 → [entry+0x14e]:=0`
  at 100b2e19–100b2e3e, end-byte trio 100b2deb–100b2dfa. End-request `f_100b34e0`: globals
  `[0x104808f0]`/`[0x104804c2]` at 100b34e0–100b34f1, yield-no-advance store 100b34f6. Pokes:
  `f_100b55d0` writes `[0x104886b8]` (100b55d9) and conditionally `[0x10485a70]` (100b55ef, gated on
  `[0x10488b00]`) before `inc exec`; `f_100b5600` zeroes the same pair; `f_100b5620` writes
  `[0x10488100]:=0` at 100b5620. BITTEST `f_100b0250`: operand fetches (`push 3` / slot re-fetch
  `push 1`) at 100b0252/100b0263, split `sar eax,5` + `and ecx,0x1f` at 100b025f/100b0271,
  set-path add-7 100b027a, jump path u16@+5 store at 100b028e. Doors: flag test
  `test byte [eax+0x120],4` at 100b6961/100b6a11, status stores `[eax+0x174] = 8/9` at
  100b696a/100b6a1a. Query thunk: `jmp 0x100b25f0` alone in `f_100b2270`. QUERYWAIT `f_100b2940`:
  result word fetched at 100b297d (`call 0x1014e1c0`), zero-path falls to yield-without-advance
  (branch target 100b2a4f storing `[esi+0x25a],1` at 100b2a48–100b2a4f… branch structure per dump),
  `ax==0xff` special stores `[0x104887f8]=0xfe` at 100b299d else `movsx/dec` store 100b29e9–100b29ed.
  Kuluu pin checked: `STATUS_EVENT_DOOR_OPEN/CLOSE = 8/9`
  (`ffxi-event/src/cue.rs`) — agrees with the two stores. Restore-clock `f_100b87b0`: gate byte
  `[0x104886da]` read at 100b87b0, callees `f_1018c4d0` (100b87bc) and `f_100940f0` (100b87c8, both
  unread singletons), visibility slot `[0x1065cb20]:=0` store 100b87c1, `inc exec` 100b87cd.
  Close-map `f_100b93c0`: helper call at 100b93c3, `inc exec` 100b93c8.
- Scheduler-helper revision (2026-10-29), `scripts/opcode-body.py` over `funcs_012.c` plus one raw
  data read: the dispatcher jump-table dwords at 0x100b73c4 taken from
  `research/xi-on-anything/generated/FFXiMain.unpacked.dll` (.text VA 0x1000 / file offset 0x2c4800),
  giving sel 0/1→100b72f0, sel 2→100b7328 (`push 0` at 100b732b, `call f_100b5510`), sel
  3/4/5/6→100b733d/7356/736f/7388 (`f_100b5210` argument pushes in order — sel4: `push 0`@100b7359,
  `push 0`@100b7362, `push 1`@100b7364; sel5 first push `1`@100b736f; sel6 1/0/1 — and each case
  pre-stores exec via `mov word ptr [esi + 0x256], <di+1>` e.g. @100b732f),
  sel 7→100b73a1 (rightmost `push 1`@100b73a4). Advance tails: `f_100b5510` 100b559f–100b55ab
  (`mov al,[esp+0x1c]`; `add word [esi+0x256],5`; yield store), entered from fetch-status failure
  (`jne`@100b5535), null entry (`je`@100b5551), id-pair (`jne`@100b5563) and bit-9/bit-7 refusals
  (@100b5573/@100b557b); the only false-return is 100b55bc–100b55cb (`mov byte [esi+0x25a],1`,
  `xor al,al`) from `test al,al`/`je`@100b558c after `call f_100d12d0`@100b5585. `f_100b5210`: tail
  100b54a8–100b54c9 (`add word [esi+0x256],0xf`; bump `add eax,2` when byte `[esp+0x30]`≠0), same
  branch structure from fetch statuses (@100b523b/@100b525e), nulls (@100b527e/@100b529b), id pair
  (@100b52bb/@100b52c5) and flag gates (@100b52d8–@100b5301); false only via `je 0x100b54df`@100b5437
  after `call f_100d12d0`@100b5430. `f_100b4e10`: tail 100b4ef2–100b4efe (`add word
  [esi+0x256],0xd`) entered from both fetch statuses (@100b4e35/@100b4e58), both null slots
  (@100b4e71/@100b4e86) and the two bit-9 tests (@100b4e93/@100b4ea1); busy select `test dl,dl` with
  `dl = byte [esp+0x20]` (the rightmost operand; sel7's first push=1) → `call dword [edx+0x2ac]`
  @100b4ec4 vs `+0x2a0`@100b4ecc, busy-true sets yield @100b4ed6 and returns false without advancing
  (@100b4ee9), leftover-yield park via `mov al,[esi+0x25a]; test al,al; je tail` @100b4edd–@100b4ee5.
  Operand getters: `f_100afac0` assembles a little-endian dword at stream `[vm+0x20] +
  word [vm+0x256] + N` (byte reads @100afad4/@100afadc/@100afae6/@100afaf0, `ret 4`); `f_100afa90`
  is its word twin; `f_100af4b0(N)` wraps `f_100af4c0(N,0)` whose bands sit @100af4d3–@100af506
  (value < 0x50 → dword at `[vm+0x260 + v*4]`; 0x50..0x7FF registers via `call f_10017d20` then
  returns zero @100af4e5–@100af4f3; ≥ 0x800 folds −0x1000 onward). Bucket anchors re-pinned at
  100b535c/100b5387/100b53af (`add eax,0x10323` / `0xe95b` / `0xdc19`; the remaining bands follow at
  100b53d7/100b53f0).

- Small-ops read pass (2026-10-29), all bodies via `scripts/opcode-body.py` over the generated C.
  `OP_IF f_100afd10`: nibble dispatch on byte `[codebase+ip+5] & 0xf`, table @100afe8c — case map
  verified slot-by-slot: 0→@100afd43 (`cmp f1,f3; je skip` ⇒ jump-on-ne), 1→@100afd72 (via tail
  @100afe78 `je target`), 2/3/4/5→@100afd8d/@100afdb0/@100afdd3/@100afdfa (`jle/jge/jl/jg target`),
  6 and 9→same entry @100afe51 (`test f1,f3; je target` — AND-zero), 7→@100afe21 (`xor edi=f3,eax=f1;
  je target` — equality by xor), 8→@100afe39 (`or edi,eax; je target` — NOR),
  10→@100afe60 (`mov edi,f1; push 3; not edi; fetch(3); test f3,~f1; je target` — bit-subset);
  nibble > 10 → @100afd5f = jump-set via `push 6; call f_100afa90`; taken tail adds 8 to ip
  (@100afe7e/@100afdab/@100afdd0/@100afdef/@100afe16). Entry issues two discarded fetches
  `push 1/push 3; call f_100af4b0` (@100afd12–@100afd1f).
- Send pair: `OP_SENDTAG f_100b63a0`, mode byte `[codebase+ip+1]`: case 0 @100b63e2 `call f_100a0510`
  → false parks (yield only, tail @100b63d8), true sets latch byte 0x104855a8 and `add ip,2`;
  case 1: latch set → park, clear → `add ip,2` then yield; default → park. `OP_EVENTPOSSET
  f_100b62b0`: case 0 @100b62fd fetches (8)(6)(4)(2) via `f_100af4b0`, converts with rdf32 constants
  0x10329d2c × 0x10330250 (heading: raw × literal then a second scale; x/y/z each × 0x1032a22c) and
  calls `f_100a05e0(4 floats)`; true → latch bytes 0x104855a8 and 0x1047faaf set, `add ip,0xa`
  (@100b638b); false → park. Case 1: either latch set → park; both clear → `add ecx,2;
  mov word [vm+0x256],cx` then yield (@100b62e8–@100b62f9). `OP_A6_SUBMAP f_100ba9c0`: case 0
  @100baa26 `call f_100fc3c0(0xeb,0,0)`, alloc fail → park; else field write via
  `f_100fc4d0(h,4,0)`, `add ip,2`, pending byte 0x104804c0 = 1. Case 1 @100baa09: pending set → park;
  clear → +2 with yield. Case 2 @100ba9e8: dword global 0x10485a28 stored via
  `f_100af310(2, value)` and `add ip,4`, then falls into the yield tail.
- Branch/staging smalls: `OP_ENTITY_VALID f_100b6400` — fetch(1) → `call f_100b3660`; true
  `add ip,5`; false jump-set via `push 3; call f_100afa90`. Wrappers `f_100b51f0` =
  `f_100b5210(0,1,0)` and `f_100b5200` = `f_100b5210(0,1,1)`. Door pair second write sites
  `mov dword ptr [eax+0x174], 0x2d/0x2e` @100b699a/@100b6a4a after gate `test byte ptr [eax+0x120], 4`
  and always-advance `inc word ptr [ecx+0x256]`. `OP_EVENT_HIDE_ALWAYS f_100b3540`: entity from
  `[vm+2]` via table 0x10480af0; `push 1; call f_1008e4b0` (this=actor) then `or ch,0x10` into dword
  `[eax+0x124]`; `inc ip`. `OP_ANIM_YIELD f_100b7520`: gate byte `[actor+0x1bc]` — nonzero →
  yield only; else `inc ip`. `OP_ZONE_READ_YIELD f_100b8630`: singleton 0x1065cb04 `call
  f_1018ba60` — al true → yield only, false → `inc ip`. `OP_ENABLE_TIMER f_100b87e0`: global byte
  0x104886da set → `call f_1018c4d0` + clear byte 0x1065cb20; always `inc ip`.
- `OP_RANGE_RECT f_100b9040`: fetch(1) via dword twin `f_100afac0`; entity `[vm+2]`, model handle
  `[actor+0xa0]`; singleton 0x10631f24: `call f_1017ad70` then bounds copy from `[model+0x34..0x40]`
  and `call f_1017ad80(handle, ptr, ptr)`; true → `add ip,7`; any null/miss → jump-set via
  `push 5; call f_100afa90`. `OP_QUERYWAIT2 f_100b2aa0`: singleton 0x1047d3e8 (guard byte 0x1047d3ec);
  query word from `call f_1014e1c0`; zero → yield only (@100b2b7c); nonzero → `inc ip`, store
  sentinel law `cmp ax,0xff` keep-0xff else dec-eax into var slot dword 0x104887f8, then refresh via
  `call f_100ae3a0`/`f_100b2a60` and widget calls @100b2b3d (`ecx=0x10621838`,
  `call f_1015e790`) / `call f_1013bf40`. Reconfirmed: `OP_TURNWAIT f_100b7dd0` (park on
  `[actor+0x12c]>>1 & 1`, edge-poke `f_1008af20`), `OP_MUSICREADWAIT f_100b6df0` (compare word
  0x10482f1c against table word indexed by byte 0x10482f1f; `call f_10037320` true parks, false
  advances one past the always-set yield), map sub-op family `f_100b8660` (loader-busy gate first;
  case widths +4 / +2 (via `add eax,2; mov [vm+0x256],ax`) / @100b86a2 rewrite `ip-6` then fetch(2),
  `call f_100fc680(2,val)==1 → +8 else +6 with yield`; default no-advance spin).
- Window/menu family pass (2026-10-29), dispatch tables dumped as raw words from the image.
  `OP_WINDOW f_100b0a00`: byte `[codebase+ip+1]`, guard `cmp ecx,0x15; ja @100b1081` (@100b0a23),
  table @100b1090 = {0→@100b0a30, 1→@100b0ab0, 2→@100b0b94, 3→@100b0c40(tail @100b0e45
  `call f_1015e910`), 4→@100b0b16, 5→@100b0c4a, 6→@100b0cca, 7→@100b0d66, 8→@100b0daa(tail
  @100b0e45), 9→@100b0db4, 0xA→@100b0dfc, 0xB→@100b0e40(tail @100b0e45), 0xC→@100b0e65,
  0xD→@100b0ead (`mov dword [0x1047bc8c],1; +2`), 0xE→@100b0ecd (=0), 0xF→@100b0eed
  ([0x1066443c]·`f_10209430(var4,var2)`+6), 0x10→@100b0f2b (`f_102094d0()`+6), 0x11→@100b0f50
  ([0x10664440]·`f_10209890(var4,var2)`+6), 0x12→@100b0f8e (`f_102098b0()`+6), 0x13→@100b0a55
  (inline copy + `cmp byte '@'; mov ' '` normalize loop over sixteen bytes), 0x14→@100b0fb3
  (fetches 2,4,6,8,0xA; four × rdf32 [0x1032a22c]; `[0x10663888]·f_101fb070(...)`; `add ip,0xc`),
  0x15→@100b1062 (`[[0x10663888]+8] non-null → yield-only spin @100b1081; else +2`)}. Pending-
  string bases 0x10488d08/0x104882b8 stride 0x100 with `byte [slot+0x17]=0` terminator writes;
  work-string get/set twins `f_100af960`/`f_100af7e0`; query-mode global byte 0x104886f4 posted via
  singleton 0x1047d3e8 `call f_1014e210(mode)`; value-widget singletons 0x1057817c/0x10578180 float
  slot +0x18, create via `f_1012c1d0(4,v)`/`f_1012c500(0,v)`, getter stores int(float) into work(+2)
  unconditionally (`push edi` after je-skip leaves edi=0 → store-zero law); chat nameplate
  singleton [0x10578478] bytes +0x65/+0x66 dword +0x68=0x80808080, widget create/lookup via
  singleton 0x10621838 `f_1015ec50(name)`/`f_1015e350(name,1,0)`, literals @0x1035bad8 /
  0x1035d2a8 / 0x1035d294.
- B6 look family (`f_100b1150`): byte map @100b1dc4 → table @100b1d8c (dumped mechanically, 24
  subs); shared advance tail `add word [vm+0x256],4` @100b1d72; spin tail @100b1d7a; look-record
  writes: +0xef (+`or al,1` into [actor+0x120]) sub 0x01; +0xfc compare-store under `or byte
  [actor+0xf4],1`; +0x10e compare-store under `or byte [actor+0xf5],2`, shared `or dword
  [actor+0x130],0x20000` @100b150f; whole-struct bodies @100b12ab (`add ip,0x14`), @100b151e
  (`add ip,0xe`), @100b1751 (exec add via register = 16) and **@100b1b14 (`add ip,0x16`, sub
  0x16)** — fields +0xfc..+0x10a/+0xfe with the flag bit set on change; model-scale float
  `fstp dword ptr [actor+0x204]` = fetch(2)×rdf32 0x10329a18 @100b19d2 (sub 0x0F) and again for
  sub 0x17 @100b1d6c with the entity from current-entity word 0x10485f7a (`add ip,4`); loading
  gate `movsx ecx, word [0x10485f7a] … shr dword [actor+0x120],9; test dl,1 → spin`, else
  `add edi,2` store (sub 0x10); method handoff `call f_1008a750(fetch(2))` on the actor (sub
  0x11); bit-4-of-[actor+0x128] set/clear pair via `cmp cl,0x12; setne; shl 4; xor-fold`
  (@100b1a61, subs 0x12/0x13 width two).
- `OP_MENU f_100b7e10`: byte `[codebase+ip+1]`, guard `cmp ecx,0x55; ja @100b8310` (@100b7e34),
  byte-map @100b8374 → table @100b8320 (both dumped twice; second pass mechanically per-sub):
  sub 0→@100b7e49 (latch bytes 0x1047fab0 / 0x10489930 set, singleton [0x10664404]·
  `f_10206540(name,0,-1,0x10)`, +2), sub 1→@100b7e82 (latch set → spin; else
  `call f_100fc3c0(0x60,0,0)`, fill dword+4 from [0x10480aa4], word +8 from 0x10482f1a, 28 bytes
  string from buffer 0x104886e0..+0xC, `call f_100fc4d0` send, +2 — alloc failure spins), sub
  2→@100b7f01 (open-byte 0x1047fab0 set → spin; else +2), sub 3→@100b7f26 (`call
  f_10094160(0x104886e0)`; `dec al/neg/sbb` boolean fold stored via setworkofs(+2); tail @100b8308
  `add ip,4`), subs 4–0xF and 0x14–0x17→spin index 0x14=@100b8310 (no-advance), sub 0x10→@100b7f4a
  (latches set; fetch(8)+1 → `f_10206540(name,0,count)`; +4 tail), subs 0x11/0x13→shared @100b7f90
  (latch clear → strlen via `call f_103128b2` stored at work(+2); set → spin; else +4), sub
  0x12→@100b7f69 (fetch(4) and fetch(2)+3, latches; tail @100b8059 then `add ip,6`), subs 0x20/
  0x21 → @100b7fb9/@100b7fcd (`call f_100b93e0` / `f_100b9470` map-window helpers, yield byte only
  — the case advance happens inside those helpers), sub 0x30→@100b7fe1 (latches set; push 0x10;
  fetch(8)+3 falls through into the shared widget tail `f_10206540(name,0,count)` then the +4
  tail — the sixteen-slot numeric-input open), sub 0x31→
  @100b8015 (latch [0x10489930] set → spin; clear → `f_100af7e0(2, buffer-addr)` string-input poll,
  +4 tail), sub 0x32→@100b8035 (fetches(4),(2)+3 like 0x12 with `add ip,6`), sub 0x40→@100b8081
  (`call f_100b8430`; `[0x10663520]·f_101e4470(1, callback @100b8460, fetch(2))`; +4 tail), sub
  **0x41**→idx d=@100b80a7 (`call f_100b8450` answered-helper: false → spin; true → globals
  0x10489934/38/3c stored via setworkofs into work +2/+4/+6, `add ip,8` — the width-eight row
  kuluu encodes and refuses as an input-wait), sub 0x50→idx e=@100b80f8 (non-null →
  `[0x10621944]·f_10163130(fetch(2))`; +4 tail), sub 0x51→idx f=@100b811f (`f_101632a0()` release
  if non-null; `add ip,2` @100b812e), sub 0x52→idx 0x10=@100b8143 (`f_101632f0(fetch(2))`; +4
  tail), sub 0x53→idx 0x11=@100b816a (null → skip; `[0x10621944]·f_10163310()` bool true → spin,
  else `add ip,2` @100b8181), sub 0x54→idx 0x12=@100b8196 (fetches 2,4,6,8; signed geometry from
  globals 0x106218b4..ba (`sub`/`movsx`, `fimul` fetch × rdf32 0x10329a18 → nearest-int via
  `f_10311c6c`), `[0x10621944]·f_10163320(w0,w1)`; `add ip,0xa` @100b82d5 — null singleton skips
  the computation but still takes ten), sub 0x55→idx 0x13=@100b82ea (null → +4 tail; else
  `f_10163340(fetch(2))` then shared `add ip,4` @100b8308).
- Five-family body pass (2026-10-29 late): bodies via `scripts/opcode-body.py`; jump tables and
  byte maps read as raw dwords/bytes from `research/xi-on-anything/generated/FFXiMain.unpacked.dll`.
  `OP_RENDERFLAG f_100bac90`: sub byte `[codebase+ip+1]`, guard `cmp eax,0x1c; ja @100bb101`
  (`@100bb101` = bare `pop esi…ret` — out-of-range advances nothing and spins, kuluu's halt being
  the faithful stand-in), table @100bb108 (29 dwords). Advance sites: +2 at @100bacc2 (entry-0 body), @100bad1d,
  @100bad4e, @100bada7, @100badd0, @100badfd, @100bae32 (several via the shared tail @100bae6d
  `add word [vm+0x256],2`), @100bae92/@100baeb7 (@100bae8b/@100baeb0 je-skip fall-ins), @100baf04,
  @100baf2c, @100bb09d; +4 at @100baf47, @100bafbd, @100bafe3, @100bb009, @100bb02f, @100bb055;
  +6 at @100bb0f8 (sub 0x1B tail). Stores: `or`/`and` bit pairs on `[actor+0x120]` bits 0 and 2
  (@100bace7 with `mov ecx,2`@100bacd3 / and-0xfffffffd @100bad14), bits 1 and 3 (@100bad45/
  @100bad96 via the door-guard path below), bits 6 and 7 (or-0x40 @100bae8b, and-0xffffffbf
  @100baeb0); `[actor+0x130]` or-2 (@100baeda with `mov ecx,2`)/and-0xfffffffd (@100baefd) pair;
  `[actor+0x128]` and-0xfffffffd @100bae21 / or-2 @100bae4d / or-0x1000000 @100baf65 /
  and-0xfeffffff @100baf8d. Door-guard halt case @100bad70: `[eax+0x174]` zero **and** byte
  `[eax+0x1bc]` nonzero → `mov byte [vm+0x25a],1` @100bad8b and return without advancing; else the
  bit-3 clear@100bad96. Global overlay flips `mov byte [0x10489900],1/0` @100baf11/@100baf25.
  Sub 7 fetch(2) → `mov dword [0x1035af3c],eax` @100baf42. Six singleton handoffs
  `f_1008aa80`/`f_1008aad0`/`f_1008ab20`/`f_1008ab70`/`f_1008aba0(fetch(2), [codebase+ip+2])`;
  door-pair helper `call f_10087120(bool)` @100bae65 (pushed 1/0 from the lookup-fail branch).
  Sub 0x19 body @100bb062: `cmp cl,0x19; sete cl … shl edx,0x13`, xor-fold mask 0x80000 against
  `[actor+0x13c]` (bit-19 set-if-equal law). Sub 0x1B body @100bb0a2: `cmp cl,0x1b; sete bl`,
  two-out fetch via `f_100afac0(push 2)`+`f_100afb00`, same bit-19 fold mask 0xfff7ffff/`shl 0x13`
  @100bb0e5.
- `OP_STATUSSET f_100bb180`: guard `cmp eax,4; ja @100bb314`, table @100bb324. The guard jump
  lands on the shared `add word [vm+0x256],cx` tail with ecx still zeroed at entry (`xor ecx,
  ecx`@100bb18b) — out-of-range subs advance zero and re-dispatch (spin); kuluu encodes no width
  there and its VM halts, the faithful stand-in. Entry bodies set `mov ecx,4/6/8` before that
  shared tail. Entry 0 (@100bb1ae): actor word at +2 → `fetch(2)` value → `mov dword [actor+0x170],eax`
  @100bb1d5, advance via `mov ecx,4`@100bb1db. Entry 1 (@100bb1ee): same shape into
  `[actor+0x174]` @100bb215, cx=4. Entry 2 (@100bb22e): `cmp eax,2; sete cl` latch (edi),
  two-out fetch via eligibility pair; fold `shl eax,0x1f / and-mask 0x7fffffff xor` into
  `[actor+0x138]` @100bb27b..@100bb288; then `call f_100aec70(actor)` — true skips; false with
  latch clear → `mov byte [vm+0x25a],1` @100bb29f return (no advance); else cx=6 @100bb2ad.
  Entry 3 (@100bb2c0): fetch(6) value, `and eax,3 / or into [actor+0x13c]` @100bb309, cx=8.
- `OP_ITEMINFO f_100b9d90`: guard `cmp ecx,0x20; ja @100b9fec`, byte map @100ba010 → table
  @100b9ff8; the out-of-range tail @100b9fec carries `mov eax,4` set at @100b9fd5/@100b9fcd-adjacent
  bodies and executes `add word [vm+0x256],ax`. Case 0 body @100b9dc3: fetch(2)/(4)/(6) into
  globals 0x104886bc/c0/c4, then the sub byte itself re-read (`mov dl,[eax+ecx+1]`) folded to
  push flag `cmp dl,3 → setne/dec → and edx,2` with record pointer 0x104886bc and id into
  `call f_1013e520`; advance `mov eax,0xa` @100b9e29. Actor-null branch @100b9e39: self variant via
  singleton 0x10578204 → `[+8]` into `f_1015e790(1)` + `f_1013bf40()`, same advance ten. Case 2
  body @100b9e66: fetch(2) id held; record block cleared (`rep stosd` six dwords at 0x104886bc);
  fetch(4)/(6)/(8)/(0xA) nibble-folded into `[0x104886c2]` (xor-masks 0xf/0xf0/0xf00 — packing
  four half-nibbles); `f_100af960(fetch(0xC))` pulls a full record → dwords at 0x104886c5/cd/e9
  region + word 0x104886d1 + byte 0x104886d3; `f_1013e520(id, record)`; advance `mov eax,0xe`
  @100b9f37. Case 0x10 body @100b9f47: fetch(4) and fetch(6)-as-bool gated on singleton
  [0x106644c0] non-null → `f_1020f1d0(w0,w1)`; advance six. Sub 0x11 body @100b9f81: byte
  `[widget+0x6d]` nonzero → `mov byte [esi+0x25a],1` @100b9f8d return (no advance); else store
  sign-extended word `[widget+0x6e]` via setworkofs(+2), advance four. Sub body @100b9fb6:
  fetch(2); zero → `f_1022a6d0()`; nonzero → `f_1022a640(value, 0)`; advance four — kuluu's
  default-four row.
- `OP_RECIPE f_100b98e0`: guard `cmp ecx,5; ja @100b9b42`, table @100b9b4c. Send bodies: alloc
  `call f_100fc3c0(0,0,0x58)` (null → fall-in to the shared no-advance tail @100b9b42 which sets
  only the pending byte); command word `[edi+0xa] = 1/2/3/4/5` immediates (@100b9930 stores case
  register bx for case 0's variant), operand words from stream +2/+4/+6/(+8)/(+A) via fetch into
  packet slots +4/+6/+8/+C/+E/+0x10/+0x12; finalize `call f_100fc4d0(handle,0,0x14)`; pending
  byte `mov byte [0x10480a90],bl` (=1); advance immediates @100b996a (+8), @100b9a16 (+0xc),
  @100b9a93 (+0xa, two bodies — case at @100b9ac5 jumps into the earlier one's operand sequence
  via `jmp @100b9a46` with command word 4), @100b9b3a (+0xe); then `mov byte [vm+0x25a],bl` —
  every send path yields. Poll body @100b997c: pending set → plain return (no advance, no yield
  write = spin-until-server); clear → `add eax,2` via the shared tail.
- `OP_RANKING f_100bbaf0`: guard `cmp ecx,9; ja @100bbd37` (`add eax,2` + pending byte — kuluu's
  default-two row), table @100bbd4c. Ranking singleton global [0x10664444; board getters
  `f_100a34b0`/`f_100a33f0/f_100a3400/f_100a3410/f_100a3450/f_100a3460` (+`f_100a3470/
  f_100a3480` on the eighteen-wide case), predicates `f_100a34d0`/`f_100a34e0`, writer
  setworkofs `f_100af310(slot=2..0x10)`]. The fourteen-wide board copy @100bbb2f (predicate
  false → jump to advance tail without writing; nine getters stored at slots +2..+C, `add word
  [vm+0x256],0xe` @100bbba8); eighteen-wide twin @100bbc08 (`… ,0x12` @100bbc9f — kuluu's
  ranking-max row). Send/select bodies push constants 1, 0xA, 0xB, 0xC, 0xD with fetch(2) through
  `call f_100a3310(selector, value)` @100bbccb then advance four + yield. Singleton-null branches
  skip the calls but keep each case's advance and yield.
- Magic pair (`f_100b4540`=0x73 wrapper `push 1; push 0; call f_100b4590`; `f_100b4550`=0xC4 sub
  byte `[codebase+ip+1]`: 0→`push 0x10`, 1→`push 0x11`, 2→`push 0x12`, each with push 1, then the
  helper; other subs fall through @100b458e with no advance). Helper `f_100b4590`: ebx := stream
  base + 3 from the caller frame; two lookups via `f_100afac0(…, push 3)` then `(+7)` each through
  `f_100afb00` (`cmp al,1` gates); scheduler-slot equality `mov edx,[ecx+0x74]` vs `[eax+0x74]`
  compared against the frame pair @100b4630/@100b4638; flags gate `shr dword [x+0x120],9; test
  dl,1` on both entities; selector fetched via `push ebx+1` fetch (@100b4678); table @100b4878
  (eighteen cells): bodies @100b4691 (`[visual_vtable+0x2d0](value, actorid, "main"=0x6e69616d)`),
  +0x2c4 variant (@100b46b6 with two pushed zeros), then +0x2cc/+0x2d4/+0x2d8/+0x2dc/+0x2e0/
  +0x2e4/+0x2e8/+0x2ec/+0x2f0/+0x2f8/+0x2f4 twins; failure label @100b4862 falls into the SAME
  advance tail `add ebx,0xb; add word [vm+0x256],bx` @100b4866.
- `OP_STRINGOPS f_100ba190`: sub byte `[codebase+ip+1]`, guard `cmp eax,0x10; ja @100ba7fc` (bare
  ret → spin), table @100ba858 dumped: subs 0x00–0x06 → bodies @100ba1d4/@100ba24a/@100ba2c4/
  @100ba365/@100ba435/@100ba4fc/@100ba576; subs 0x0A–0x10 → the SAME bodies offset by one
  (@100ba1c2/@100ba238/@100ba2b2/@100ba350/@100ba423/@100ba4e7/@100ba564 — each body's entry
  begins with the wide-path prologue). Widths inside: `push 8; fetch` result zero → narrow (`mov
  ebp,8`@100ba1d4 / else `mov ebp,0xa`@100ba1c6), advance via shared register tail (@100ba226,
  @100ba314, @100ba4d5); registry pair dwords 0x104897f8 (handles) / 0x104896e8 (values), store
  `mov dword [idx*4+0x104897f8], streamword(+2)` @100ba2fe with `[vm+0x1c]` mirrored to the values
  table; index ≥ 64 path `cmp ax,0x40` → literal blocks 0x1035d388/0x1035d364/0x1035d340/
  0x1035d31c/0x1035d2f8 through `call f_10017d20`; string-slot store via setter twin
  `f_100af7e0(2, src)` with source at stack dwords (sixteen bytes) and slot index ×2 (`shl …,4`
  then ×2 addressing into the handle); compare bodies @100ba646 (four-candidate walk over inline
  words +5 (+N×0x10), mismatch → `push 7; call f_100afa90; mov word [vm+0x256],ax` label-jump
  @100ba7f0) and @100ba741 (two registry strings, inline nibble byte +6 selects one of four,
  mismatch → push 7 jump; match advances nine @100ba840); arithmetic case @100ba5f9 reads two
  stream words via the code-redirect helper pair then `call f_100b0510(id_a, id_b)` with base+6 —
  kuluu's width-nine row; sweep posture from `cargo run -q --release -p ffxi-event --example
zz-event-sweep` (refused: 71×96, 31×16, 5A×11, 8C×7, B3×2, CC×2 — no 0x9D).
- Item card, pair dispatchers and the flags-word family map (2026-10-29 late). Fetch helper
  `f_100afac0` returns the LE dword at `[data+ip+idx]` (`mov ax,[ecx+0x256]` @100afac5, byte rebuild
  100afad4..100afaf5); actor resolver `f_100afb00`: special-id window `lea esi,[eax-0x7fffffc0]`,
  `cmp esi,0x39` @100afb0b with jump table @0x100afc08 and byte map @0x100afc34; existence gate
  `call f_1008b380` @100afbf8 against `[eax*4+0x10480af0]`. The eligibility dword is entity +0x120 bit 9
  (gate sequence @100b8a41..).
- Force-setters. A4 `f_100ba8d0`: subject word `[ecx+2]` → table @100ba8d6; operand byte
  `[edx+esi+1]`@100ba8f1, `shl ebx,0x1a`@100ba8fb, `and ebx,0x4000000`@100ba901, xor-store into
  `[eax+0x12c]`@100ba909. A5 `f_100ba920`: same shape via `shl ebx,0xb`@100ba94b / mask @100ba951,
  store @100ba95a. C0 `f_100ba970`: operand through `f_100af4b0(1)` (chains into `f_100af4c0`),
  `and eax,1`@100ba98f, `shl eax,0xc`, store @100ba9ab. Turn-wait gate: `[ecx+0x12c]`, `shr edx,1`,
  `test dl,1` @100b7de4.. yield-byte park @100b7df1; mover in-flight or-in is
  `or dword ptr [eax+0x124],0x20000` @100b56be — second word.
- 0x93: operand fetch at @100b9d35 (`f_100af4b0`, idx one), `test ax,ax`@100b9d3a; close path calls
  `f_1015e790` @100b9d51 (this = static object @0x10621838, record argument `[manager+8]`, manager
  global [0x10578204]) then the whole-object reset `f_1013bf40` @100b9d58 (sentinel tail filled by
  `rep stosd` nine dwords from +0x383); open path calls mode-0 opener `f_1013e520` @100b9d73 with the
  operand as item id — the same call sites the menu-docking record lists at RVA 0xB9D51 / 0xB9D73.
  Staged-record guard bytes inside `f_1015e790`: [record+0x6e] init-once @1015e7a5, [record+0x70] park
  byte when `[record+0x10]==0xe` @1015e7b4, [record+0x72] cleared before the staging call
  `f_1015e6e0` @1015e7cb.
- AD nibble dispatch: `mov cl,[ecx+edx+1]`@100bb43c, `and ecx,0xf`@100bb440, `cmp ecx,9`@100bb443,
  jump table @0x100bb5f4 (ten entries); literal push `0x6e69616d` ("main") once per case; handle method
  calls run from `[ebx+0x2c4]` through `[ebx+0x2ec]`; gate-failure tail advances twelve at @100bb5e2.
  AE sub table @0x100bb990 (`cmp eax,8`@100bb63b); +472 stores: fetched word
  `mov word ptr [edx+0x1d8],ax`@100bb6e3, constant `mov word ptr [eax+0x1d8],0x17`@100bb6f7; link set
  and clear are `mov dword ptr [eax+0x798],ecx`@100bb840 / `mov dword ptr [eax+0x798],0`@100bb8b4;
  advance sites eight @100bb6ea/@100bb846 and six @100bb669 (case zero).
- Small handlers (2026-10-29 late). 0x95/0x96 method pair: word-fed `f_10087d80` @100b9c4b / bare
  `f_10087db0` @100b9c84, shared refresh tail `f_10092900` @100b9c5d/@100b9c96; the 0x96 increment sits
  @100b9c9b. 0x97: prep helper `f_100ba040` @100ba0c3, singleton `[0x1065cb04]` dword stores +0x138/+0x13c
  @100ba0d9/@100ba0ec. 0xA8: gate `[0x10663888]` @100b97bf; staging `f_100b2570(4, word sx, -1, 0, 0)`
  @100b97d8; singleton methods `f_101f8da0`/`f_101f8e30` (literal string pointer [0x103d3c84])
  @100b97ec/@100b97ff; close-map sibling `f_100b2a60` @100b9808. 0xD3: no-advance exit on non-zero sub
  @100b7158→100b71d0; predicate `call f_1002c8f0` with class-table literal 0x10330ebc @100b71aa..,
  follow-on handle call `f_100ce070` @100b71c2; running advance six @100b71c7. 0xC2: selector ladder
  `dec/dec je` @100bbf9d..@100bbfa1; queries `f_100e9850(0)` @100bbfdb and `f_100e98b0(word,0)` @100bbfbc;
  registry stores `call f_100af310` with index two/four @100bbfe8/@100bbfc9; jump-set exec=ip+2
  @100bbfa3..@100bbfa6. 0xC3: range gate (`jl`/`cmp eax,8 jge`) no-advance path @100bc00c/@100bc011 →
  pop tail; slot math `lea [eax+eax*4]` into base [0x10488210] @100bc013..@100bc01b (stride five dwords);
  block call `f_100af960(3)` @100bc022; dword copy +0..+C, word@+5 into slot dword four @100bc048.
- Census tail two (2026-10-29 late). BF `f_100bbd80`: ceiling `cmp eax,0x60 ja` @100bbda3 → pops-only
  exit; byte map @0x100bbf1c (classes zero/one/two/four), jump table @0x100bbf08 targets 100bbdb8 /
  100bbdfe / 100bbe5a, default target **100bbf02 = pops with no add**; registry getters `f_100f97c0`
  @100bbdbb, `f_100f97f0` @100bbe01, `f_100f9820` @100bbe5d (a fourth getter `f_100f9850` @100bbec3 sits
  past the last ret and is not a map target — dead or foreign); twelve-byte entries via `lea [eax+eax*4]`
  + ×4 addressing; out-of-range entry trace literals 0x1035d4d8 / 0x1035d49c / 0x1035d45c / 0x1035d420 /
  0x1035d3e8 / 0x1035d3ac through `call f_10017d20`; stores af310 slot six @100bbef5 (add eight @100bbefa)
  and slot eight @100bbeab (add ten @100bbeb4). B1 `f_100bba70`: mode global `[0x10456a28]` dword+0x18;
  bodies add-four @100bba9d / @100bbabc / (third body store @100bbad9..), no-advance exit for other
  selectors falls through to pop tail; work slot two via af310 `push 2` sites. B9 `f_100b9820`: fetches
  +2/+4/+6 @100b9854/…; gate `[0x10663888]` @100b9871; staging `f_100b2570(4,a,b,0,0)` @100b9890;
  methods `f_101f8da0` @100b98a4 / `f_101f8e30` (literal [0x103d3c84]) @100b98b7; close-map sibling
  `f_100b2a60` @100b98c4. AA `f_100babe0`: getters `f_10103a80/…90/…b0/…e0/…b10/…b30/…b60`; +0x376
  adjustment @100babf6; af310 slots 3..15; add seventeen @100bac79. AF `f_100bb9c0`: singleton via
  `f_10015250`, float fields [eax+0x44/48/4c] and [eax+0x50/54/58], x87 loads, conversion helper
  @10311c6c (same tail call as 57 `f_100b0350` @100b0362 after `call f_10014cf0`), af310 slots 2/6/4.
  BE `f_100b3e20`: index from word `[ecx+0x258]`, `add eax,2; shl eax,5`, dword load [exec+idx×32],
  slot one. D9 `f_100b7be0`: `sete` → byte global [0x10463924] @100b7bf6. 9E `f_100ba8a0`: dword global
  [0x104898fc] @100ba8b6.
- Corpus census (this pass): `cargo run -q --release -p ffxi-event --example zz-opcode-gap`,
  header line `event DAT corpus: zones=299 threads=186815`; executed-site counts per op: 0xCC
  total 3588 over 101 zone sites, 0xAB 2962/158, 0xAC 653/101, 0x7F 86/50, 0x8C 40/10, 0xB3
  28/6 (linear thread walks from entry points; jump targets not followed).
- Queue close (2026-10-29 late), bodies via `scripts/opcode-body.py` over the generated C; both
  handlers' jump tables read as raw bytes out of the unpacked image through its own section table
  (.text file offset 0x2c4800 at VA 0x10001000 — pefile's rva→offset is wrong on this build, which
  is why earlier reads of these tables misfired; anchors: the five B7 targets are exactly the
  disassembly's case addresses).
  - Dispatch stubs pinned slot-by-slot from table @100bc960 for all eight queue ops (op→handler:
    04→f_100b0120, 83→f_100b90e0, 85→f_100b9120, 91→f_100b9b70, 9C→f_100ba100, A2→f_100b4970,
    B7→f_100afee0, D8→f_100bc060).
  - 0xB7 `f_100afee0`: selector byte `[data+ip+1]` @100afeeb.., `cmp eax,4 ja 0x100b0100`, jump
    table dumped from VA 0x100b0108 → [@100aff0b/@100aff6a/@100b0000/@100b0055/@100b009a]; default
    arm @100b0100 pops with no exec write. Case zero: fetch pair via `f_100afac0` (push 2),
    resolver `f_100afb00`, entry table `[edx*4+0x10480af0]` @100aff34, mover pointer
    `[eax+0xd4]` gate @100aff3f, stream word via `f_100af4b0(8)` @100aff49 into `f_100af310(6,…)`
    (mover-block work slots) @100aff57; advance ten @100aff5c on every path. Case one: fetch+resolve
    @100aff78, name-pair branch `cmp word [eax+0x210],0` @100affa2 → copy from +0x214 when positive
    else from +0x7c, literal fallback pointer 0x103d3c84 stored as the pair source @100affe4;
    mover-copy helper `f_100af7e0(2,…)`; eight @100affbb/… Case two: store zero @100b001f when
    resolver returns non-one, else fetched dword @100b0039; eight. Case three: sentinel 0xFFFF when
    unresolved @100b0072 else fetch-dword, `f_100af310(2,…)`; eight. Case four: fetch @100b00c6,
    range gate (`jl`/`cmp eax,8 jg`) @100b00cd, word store `mov word [eax*2+0x10480a94],di`
    @100b00d4, trace literals 0x1035d238 / 0x103d3c84 through `f_10017d30` on out-of-range
    @100b00ea; eight via the shared tail.
  - 0xD8 `f_100bc060`: entry loads `mov ebx,6` @100bc076; fetch pair + resolver gate failures jump
    to shared tail @100bc23d (advance six). Jump table dumped from VA 0x100bc24c →
    [@100bc0e3/@100bc12d/@100bc169/@100bc1a5/@100bc1bd]. Case zero copies record dwords
    +0x34/+0x38/+0x3c/+0x40 into mover-block fields +0x150/+0x154/+0x158/+0x15c (mover state via
    `[edi+0x260]`) @100bc0e3..; cases one/two fetch stream word (+6) and store scaled
    (`fild` × rdf32 [0x10329d2c]=6.283 (author-rounded two-pi literal) × rdf32 [0x10330250]
    (= exactly 1/4096)) into +0x150/+0x154 @100bc156/@100bc192, advance-register loads `mov ebx,8`
    @100bc144/@100bc180; case three stores only +0x158 via shared coprocessor tail @100bc225;
    case four chains all three (fetches at +6/+8/+0xa), `mov ebx,0xc` @100bc21c.
  - Audits: 0x91 store `fmul dword ptr [0x10329cec]` (= exactly 10.0) then `fstp [eax+0x16c]`
    @100b9b8f/@100b9b95 (subject from `[esi+0x260]`). 0x83 getter
    `f_101039e0` = `call f_100dc130; add eax,[0x104e0af8]`, slot-one store via `f_100af310(1,…)`
    @100b90ed. 0x85 callee `f_101d6110`: singleton object 0x10621838, gate global `[0x106631d4]`
    (+0x1c word compare one), flag postings via `f_1015e350`/`f_1015ecc0` (mask 0x20000) with
    literal strings @0x1037ec94/@0x1036fff0/@0x10371648. 0x9C: singleton `[0x10456a28]` state byte
    +0x18, dword+8 ∈ {2,3} → one @100ba109..; ladder @100ba123.. storing (1,1) default @100ba130 /
    (4,1) @100ba14a / (3,1) @100ba163 / (2,1) @100ba17c via `f_100af310`; every advance three
    @100ba139..@100ba184. A2 helper `f_100b4af0`: magic selector 0xc7ef pushed by wrapper
    @100b4970; identity re-check: actor A from the first stream operand (fetch width three),
    actor B from +7 (`push 7; call f_100afac0` @100b4b25); [entryA+0x74]/[entryB+0x74] staged and
    [entryA+0x78] compared against the first operand dword, [entryB+0x78] against the second
    (compares @100b4b70..@100b4b90); eligibility bit (`shr edx,9; test dl,1` both sides)
    @100b4b92/@100b4ba0; fetch(+0xb) then fetch(+0xd);
    value 0x77f0 branch `call f_100b4350` @100b4bdd — an anchor ladder (add 0x6551 / +0x9adb for
    ≥600 / +0x12fe5 for ≥900) in the scheduler-id bucket family (`f_100af4b0`-chain of the
    scheduler-helper section); intersection call `f_10062ed0(a,b,c,d)` @100b4be8; true → pending
    byte `[esi+0x25a]=1` with NO advance @100b4bf5; false/any gate failure → shared
    advance-fifteen tail `add word [esi+0x256], 0xf` @100b4c05. Selector constant 0xc7ef appears in exactly three
    call sites image-wide (funcs_011/012/032 — the wrappers of this family).
  - Sweep posture unchanged across all arming this pass: `cargo run -q --release -p ffxi-event
    --example zz-event-sweep` → ended 60830 / cancelled 1772 / refused 107 (71×96, 8C×7, B3×2,
    CC×2) / spun 9 / step-limited 84. Ledger census after this pass: MATCH=178, MATCH-ADVANCE=2,
    NO-ARM=26, WIDTH-ONLY=13. (Recount after the reason-close pass below: MATCH 184,
    NO-ARM 20, WIDTH-ONLY 15 — the difference is audit verdicts on effectless handlers plus two
    new arms in the width-only family, `OP_WORK_SLOT_TRIPLE_READ_AF` and
    `OP_STAGED_SLOT_LOADER_C3`; whole-corpus sweep counts unchanged by both.)
  - Fresh gap-census run this pass (`cargo run -q --release -p ffxi-event --example
    zz-opcode-gap`, same corpus line zones=299 threads=186815): linear-walk encounter counts —
    D8 124/24, B7 21/8, A2 13/7, 9C 86/22, 83 101/50, 0x04 4/4, 91 2/2, 0x85 0; the tool walks
    with `opcode_meta::sub_size` widths so encounters stay aligned regardless of arm state, and
    it lists arms by scanning vm.rs source constants (ops armed through opcode_meta only still
    print as rows). Operand samples (`--sample 0xB7,0xD8,0xA2,0x9C,0x83`): D8 records land size
    eight with selector bytes `01`/`02`/`03`; A2 fifteen-wide records carry special-window actor
    words (0xfff8…/0x7fff…) and inline four-character tails; 9C/83 three-wide records show the
    handlers ignore their trailing operand bytes entirely — matching both handlers' zero-stream-
    read laws.
  - **Gap-census tool fixed (2026-10-30); every earlier gap-row count from this example is superseded**
    (`ffxi-event/examples/zz-opcode-gap.rs`). The arm detector had four parsing bugs, all in that
    example and none in the DAT corpus:
    1. constant values were taken as "the first run of hex digits" of the initializer, so `0x46` read
       as `0`; every `OP_*` value collapsed to zero and armed opcodes printed as gaps;
    2. only bare `const OP_` declarations matched, missing the width-table constants' `pub(crate) const`
       visibility qualifier in `opcode_meta.rs`;
    3. its constant pool was vm.rs alone — arms whose name lives in `opcode_meta.rs` were invisible to it
       (the ledger script took the same fix earlier, recorded above under "Ledger tooling");
    4. the arm matcher missed shared arms (`OP_SMOVE | OP_CODE_MOVE2 =>`) and multi-line pattern heads.
    Measured effect: gap rows went **217 → 33** (fixes 1–3) → **25** (fix 4, which additionally requires
    every bar-split side of a dispatch line to look like an opcode pattern while keeping `use` lists out).
    Re-run (`cargo run -q --release -p ffxi-event --example zz-opcode-gap`) — the honest gap list,
    valid opcodes with no arm in vm.rs/scene.rs, linear-walk encounters/zones over zones=299
    threads=186815:

    ```
      0x52 OP_ENDLOADSCHEDULER_MAIN 79964/211   0x1F OP_MOVE_TEST              69706/205
      0x55 OP_WAITLOADSCHEDULER     14723/236   0x93 –                         13454/147
      0x7B –                          9392/185  0xAD –                          1462/106
      0x30 –                           444/151  0x95 –                            267/54
      0xA8 –                           192/21   0x96 –                            166/49
      0x97 –                           150/34   0x83 –                            101/50
      0x57 –                            86/34   0x9C –                             86/22
      0xC2 OP_MOGHOUSE_VISIT           46/13    0xD9 –                             40/12
      0xAA –                            27/22   0xB9 –                              27/1
      0xA2 –                            13/7    0x04 –                               4/4
      0x6D –                             4/1    0x9E –                                4/1
      0x91 –                              2/2   0x85 –                                 0/0
      0xBE –                              0/0
    ```

    **This 25-row list is itself superseded two hours later — see the next bullet.** Unaffected by that
    round: the linear-walk **encounter counts** (the walk advances through `opcode_meta::sub_size`
    widths, independent of arm state) and the whole-corpus sweep posture (ended 60830 / cancelled 1772 /
    refused 107 / spun 9 / step-limited 84 — identical before and after).
  - **Gap-census detector corrected again (2026-10-30, same day): the true gap list is 22 rows.** The
    25-row run above was cross-checked against `scripts/opcode-parity-ledger.py`'s verdicts before being
    trusted, and three of its top rows disagreed with the ledger — 0x52, 0x1F, 0x55. They were detector
    artefacts; two more flaws remained in that example:
    - **its own test module read as production text.** vm.rs declares `OP_MOVE_TEST` (value 0x1F) at
      vm.rs:9204, inside `#[cfg(test)] mod tests` (which begins at vm.rs:4219). The constant pool took
      that name for 0x1F; a test-only constant never appears in dispatch, so the opcode printed as a
      gap while its real arm — scene.rs's `OP_MOVE | OP_CODE_MOVE2`, scene.rs:959 — went unqueried.
      Fix: read only `without_tests` text (everything before a file's first test attribute).
    - **one alias per opcode, and multi-line pattern heads dropped.** The old code resolved each opcode
      to the *first* constant name it found for that value and asked whether that one was armed; 0x52
      and 0x55 are arms whose head line is a bare constant (`OP_ENDLOADSCHEDULER_MAIN`,
      `OP_WAITLOADSCHEDULER` at vm.rs:2740 / vm.rs:2500), which the "line must contain `=>` or begin
      with `|`" filter discarded, continuation lines being nameless. Fix: collect every alias per
      opcode across vm.rs / vm/scene.rs / opcode_meta.rs (test text excluded) and treat the opcode as
      handled when any alias is used for more than its own declaration.
    Measured sequence of gap-row counts as each flaw fell: **217 → 33 → 25 → 22.** The example also now
    prints `walks not reaching thread end` in its corpus header (measured 0, so the encounter counts are
    full linear coverage) and a `cw` marker separating ops whose width table answers per-selector widths
    (deliberate skip family) from those with only a fixed width.
    Final honest gap list (`cargo run -q --release -p ffxi-event --example zz-opcode-gap`, zones=299
    threads=186815, parse-failed DATs 0, partial walks 0; op / kuluu constant name / encounters / zones):

    ```
      0x93 –                   13454/147    0x7B –                    9392/185    0xAD –            1462/106
      0x30 –                     444/151    0x95 –                     267/54     0xA8 –              192/21
      0x96 –                     166/49     0x97 –                     150/34     0x83 –              101/50
      0x57 –                      86/34     0x9C –                      86/22     0xC2 OP_MOGHOUSE_VISIT 46/13 (cw)
      0xD9 –                      40/12     0xAA –                      27/22     0xB9 –               27/1
      0xA2 –                      13/7      0x04 –                       4/4      0x6D –                4/1
      0x9E –                        4/1     0x91 –                        2/2     0x85 –                 0/0
      0xBE –                         0/0
    ```
    Reconciled against the ledger, which is the bucket that carries verdicts (MATCH 184 / NO-ARM 20 /
    WIDTH-ONLY 15) — the two tools measure different questions and both answers stand:
    - of these 22 rows, 16 are ledger NO-ARM: `57 7B 83 91 93 95 96 97 A2 A8 AA AD B9 BE C2`;
    - 6 are ledger **MATCH by recorded audit** (`0x30 0x04 0x6D 0x85 0x9E 0xD9`) — effectless or
      stub handlers where the width skip *is* the retail response, so they are correctly absent from a
      "kuluu has no code here" list only if that list means "no handling at all", not "no arm";
    - 4 ledger NO-ARM rows (`87 88 8C B3`) do **not** appear in the gap list, and correctly so: kuluu
      mentions them in an arm — `OP_FRIENDPASS_87 | OP_FRIENDPASS_88 | OP_RECIPE | OP_RANKING =>` at
      vm.rs:2271 — but that arm is the declared refusal (`unsupported_query_workflows_do_not_send_or_
      fabricate_results`). Named-in-an-arm and un-emulated are different claims; this census answers
      "does kuluu's VM mention the opcode", the ledger answers "is retail's behaviour modelled".
    The same suspicion applied to the ledger itself (its arm rule could have counted vm.rs's test-module
    constants as arms), so both detectors were run over all 219 slots: **zero disagreements**. Only two
    names in the crate exist solely inside a test module — `OP_MOVE_TEST` at 0x1F and `OP_SPEED_TEST`
    at 0x32 (vm.rs:9204/9205) — and both opcodes are genuinely armed under their production aliases
    (`OP_MOVE` scene.rs, used eleven times; `OP_MAIN_SPEED`/`OP_SPEED`). So MATCH=184 contains no
    test-shadowed arms.
    Nothing about retail changed in any of this, and no shipped behaviour changed either: the corpus
    encounter counts, the sweep posture (re-measured after the fixes: ended 60830, ran past end 2,
    cancelled 1772, refused 107, spun 9, step-limited 84) and all 254 crate tests are identical across
    every detector fix.
- 0x7A / 0xB5 bodies (2026-10-29 late). `f_100b3a50`: ceiling `cmp eax,5 ja` @100b3a6d; jump table
  dumped from VA 0x100b3ca8 → [@100b3a76/@100b3ab1/@100b3afd/@100b3b97/@100b3bf8/@100b3c61].
  Cases zero/one call helper `f_100b3dc0(token, dword)` @100b3a9c/@100b3ae8 — the helper chains
  `call f_100b3660` (entity-valid), record presence, eligibility bit (`shr edx,7; test dl,1`) and
  sign byte before handing off to mover method `f_100b3830`; advances six @100b3aa4 / seven
  @100b3af0. Case two: entity word +1 → table, `[eax+0xd4]`→[+0x260] gates @100b3b27..@100b3b4d,
  child-attach writes `mov word [esi+2],dx` (current entity := fetched id) @100b3b53,
  `mov dword [esi+8],ecx` (operand slot := second fetch) @100b3b57, and
  `mov eax,[eax]; mov edx,[edx+0xd4]; mov edx,[edx+0x260]; mov dword [esi+0x260],edx`
  (thread mover-state field := child's state block) @100b3b5a..@100b3b68, then the counter
  increment `[mover+0x264]` inc/store @100b3b70..@100b3b83; pending byte set with no advance on
  any gate-fail path (@100b3b8b). Case three: mover counter load `mov ecx,[eax+0x264]` @100b3bb2,
  `dec`/clamp-to-zero @100b3bb8..@100b3bc1
  (`[eax+0x264]` where eax = [entry+0xd4]), then restore from the thread's own saved slots —
  `[esi]/[esi+4]/[esi+0x25c]` into `[esi+8]/[esi+2]/[esi+0x260]` @100b3bd7..@100b3be6 — pending
  set; no code in this handler writes the saved trio (writer unnamed, Not established #15).
  Case four: fetch width three (bytes +2/+7 read at @100b3c2a/@100b3c2d), predicate
  `call f_100b3cc0(4 args)` @100b3c3b, `sub eax,2; jne` → advance eight @100b3c54, equals-two →
  pending-no-advance @100b3c48. Case five: predicate `f_100b3d70(word)` @100b3c89 same park law
  via shared advance-six tail. `f_100b10f0`: sub gate `test dl,dl jne` → pending byte @100b113c;
  resolver from word +2 @100b110d..; fetch `call f_100af960(2)` @100b111c; name copy callee
  `f_10093430`: gate `cmp word [ecx+0x210],0` @10093432, `rep movsd` six dwords into `[ecx+0x7c]`
  @1009344a, terminator `mov byte [ecx+0x94],0`; advance four @100b1134. Pins added to both
  sub-width tests: (0x7A subs two/three/four → six/two/eight; ceiling selector six stops),
  (0xB5 selector one stops).
- The three client-global stores and the menu-window op (2026-10-30, re-read from this build's generated C
  in `funcs_012.c`; every site below quoted verbatim):
  - **0x30 → `f_100b5620`**: `mov byte ptr [0x10488100], 0` then `inc word ptr [ecx + 0x256]` (width one).
    Cell 0x10488100 elsewhere: set to one by `f_100aed00` (@100aed4f, `mov byte ptr [0x10488100], 1`) and
    read by `f_100ae3f0` (@100ae84c, `mov al, byte ptr [0x10488100]`). Callers of that pair: `f_100aed00`
    ← `f_10096640`, `f_100a0200`, `f_100a02d0`, `f_100a03c0`; `f_100ae3f0` ← `f_10093b70`, `f_100ae960`.
    Three sites image-wide, so the cell is a real flag rather than a scratch byte.
  - **0xD9 → `f_100b7be0`**: `mov edx,[ecx+0x20]` (codebase) / `xor eax,eax` / `mov ax,[ecx+0x256]` (ip) /
    `mov bl, byte ptr [edx + eax + 1]` (inline operand byte at +1) / `test bl,bl` / `sete al` /
    `mov byte ptr [0x10463924], al` / `add word ptr [ecx+0x256], 2`. Cell 0x10463924 elsewhere: written by
    `f_100355f0` (@100356d8, `mov byte ptr [0x10463924], bl`) and compared at `f_10038330` (@10038364,
    `mov al, byte ptr [0x10463924]` feeding a `je`).
  - **0x9E → `f_100ba8a0`**: same codebase/ip setup, `xor ebx,ebx`, `cmp byte ptr [edx + eax + 1], bl`,
    `sete bl`, `mov dword ptr [0x104898fc], ebx`, `add word ptr [ecx+0x256], 2` — so the stored value is
    exactly one or zero, and it is a *dword* cell. Cell 0x104898fc elsewhere: written by `f_10094190`
    (@10094205), `f_100ae3f0` (@100ae817, from edi) and `f_100aed00` (@100aedab, from ebx).
  - **Store-free by body, not by assumption**: `f_100b0120` (op 0x04) is one instruction
    (`add word ptr [ecx+0x256], 3`) and `f_100b7850` (op 0x6D) likewise with seven — no memory write in
    either, so the width skip genuinely equals retail there.
  - **0x85 → `f_100b9120`**: pushes zero and calls `f_101d6110`, then `inc word ptr [ecx+0x256]`. That
    callee is a named-menu-window poster on UI singleton 0x10621838 through method `f_1015e350(str, 1, 0)`:
    arg==0 branch (@101d6116 falls through) → read global `[0x106631d4]`, null-gate, then
    `cmp word ptr [eax + 0x1c], 1` @101d6121 → equal: push string 0x1037ec94 = `"menu    rmlo1   "` and call
    (@101d612c..@101d6136); not equal (@101d613c): `push 0x20000; mov ecx, 0x10621838; call f_1015ecc0` —
    if that flag/window test reports absent (`test al,al; jne`) → push string 0x1036fff0 =
    `"menu    myroom  "` and call (@101d614f..@101d615d). arg!=0 branch (@101d6163) runs the same
    flag-window test first, then pushes string 0x10371648 = `"menu    mogext  "` and calls
    (@101d6176..@101d6184). All three strings read out of this build's image .rdata (pefile,
    ImageBase-relative).
  - Ledger recount after arming the three stores and downgrading 0x85: **MATCH 183 / NO-ARM 21 /
    WIDTH-ONLY 15** over 219 slots, zero thin non-MATCH rows (the old 184/20/15 included 0x85 as a false
    MATCH). Gap census after the arms (`cargo run -q --release -p ffxi-event --example zz-opcode-gap`, same
    corpus line zones=299 threads=186815, partial walks 0) — **19 rows**, exactly the previous list minus
    0x30/0x9E/0xD9:

    ```
      0x93 –                13454/147   0x7B –                 9392/185   0xAD –             1462/106
      0x95 –                  267/54    0xA8 –                  192/21    0x96 –               166/49
      0x97 –                  150/34    0x83 –                  101/50    0x57 –                86/34
      0x9C –                   86/22    0xC2 OP_MOGHOUSE_VISIT   46/13    (cw) 
      0xAA –                   27/22    0xB9 –                    27/1     0xA2 –                13/7
      0x04 –                    4/4     0x6D –                     4/1     0x91 –                  2/2
      0x85 –                      0/0    0xBE –                        0/0
    ```
- **Two bugs in the first version of `scripts/opcode-effect-table.py`, both caught by its own `--check`
  fence rather than by rereading code.** Image addresses in this build are eight hex digits; my patterns
  matched seven, which (a) never matched a call target, so 192 handlers printed as call-free, and (b)
  mislabelled every absolute client-global store as register-relative. The drift report also surfaced
  opcode 0x24 (`f_100b2270` → `f_100b25f0`, a one-jump thunk), which that version counted as "no stores,
  no calls" — judging an effect from a stub is precisely the error the table exists to prevent, so it now
  walks thunk chains and prints both hops.
