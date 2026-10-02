# Event control, clock and ordinary door cues (retail-2026-09)

Inspected 2026-10-02 against the registered retail install, patch
`30260904_1`. FFXiMain.dll SHA-256:
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Freshly unpacked `.text` SHA-256:
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
The fresh unpack matched the saved dump byte for byte. Image base is
`0x10000000`; all code addresses below are RVAs. This is static binary
inspection, not a live retail capture.

## Player control

The dispatcher table at RVA `0xBC960` resolves opcode `0x20` to handler
`0xB34C0`. It writes the raw operand byte at instruction offset 1 to the
global byte at VA `0x10482F18`, then advances by two bytes. The movement
path at RVA `0x9652E` tests this same byte and rejects movement through
`0x96601` when nonzero. The outbound position-packet path at `0x98530`
also skips packet construction through `0x98608` when it is nonzero.
The controller at `0x157E2E` tests it before accepting its action path.
Thus zero releases this control gate and a nonzero value locks it;
these findings do not enumerate every command permitted during events.

## Clock and date

Opcode `0x77` resolves to RVA `0xB8710`. It reads work operands at offsets
1 (hour) and 3 (weather). Each has an independent `255` no-change sentinel.
When changing the hour, it disables the live ticking flag through
`0x18C4F0`, sets the hour through `0x18BF50` and minute zero through
`0x18BF00`, then updates the clock object. Weather uses a distinct call
to `0x940B0`; Kuluu's existing hour-only implementation does not implement
that weather override.

Opcode `0x78` resolves to RVA `0xB87B0`. If the clock override is active,
it enables ticking through `0x18C4D0` and clears the override flag. It also
calls the separate weather-restoration helper `0x940F0`. Clock enable
sets the ticking byte at VA `0x10377C6C` to 1 and calls `0x18C2B0`, which
resynchronizes clock components from the current live time. Clock disable
sets that byte to 0. These findings support releasing a clock hold back
to live time rather than continuing from the frozen instant.

Opcode `0xA9` resolves to RVA `0xBAB50`: it reads work offset 1, converts
it through `0x18BE70` (exactly input times seven), zeroes the local clock,
sets the day through `0x18BFA0`, and sets minute 30 through `0x18BF00`.
The day helper uses 5,184,000 clock units per day, and the minute helper
uses 3,600 units per minute. Opcode `0xC9` uses `0xB87E0` to enable the
clock and clear the override, without `0x78`'s weather-restoration call.

## Ordinary doors

Opcode `0x4C` resolves to RVA `0xB6950` and writes actor offset `0x174`
to 8. Opcode `0x4D` resolves to `0xB6A00` and writes 9. Both skip the write
if the event actor is absent or actor offset `0x120` has bit 2 set, then
advance one byte. Opcode `0x4F` resolves to `0xB69B0`, reads work offset 1,
writes that value plus 18 under the same guard, and advances three bytes.
Renderer change deduplication is not evidence that this native bit guard
has been reproduced. The current trace establishes the status writes,
not the complete status consumer or the meaning of that guard bit.

## Scope limits

Opcode `0x8D` at RVA `0xB9330` passes its separate signed work operand
at offset 3 as a submenu argument, unlike `0x89` at `0xB9300`, which passes
the sentinel -1. Property-free MapOpen cannot faithfully represent both.
Opcode `0x6C` at `0xB7630` immediately advances nine bytes when its actor
is unresolved or actor offset `0x120` lacks bit 9. Otherwise it interpolates
the target alpha byte from work offset 5 over signed-word work offset 7,
promoting a zero duration to one. An unconditional actorless VM wait does
not reproduce this resolution policy.

Live retail observation remains unavailable on the locked desktop. Actual
Kuluu pixels and runtime cleanup require separate production captures;
matching tests alone do not establish presentation parity.
