# Event emote start and wait policy — 2026-10-02

## Build and method

Registered `retail-2026-09`, patch `30260904_1`. `FFXiMain.dll` SHA-256:
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
The freshly unpacked text SHA-256 is
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`;
it matches the cached text byte for byte. Image base VA `0x10000000`.
All code addresses below are RVAs; explicitly named globals are VAs.

Resolve the dispatch table at RVA `0xBC960` for opcodes `0x63`, `0x6E`
and `0x99`, then follow their direct thunk calls. Reproduce using this
repository's `ffxi_disassembly/common.py` `Image`, an installed DLL and
32-bit Capstone. The old XiEvents opcode pages are locators; these findings
come from the current binary. Temporary trace script/output:
`/private/tmp/kuluu-retail-emote-handlers.py` and
`/private/tmp/kuluu-retail-emote-handlers.log`.

## Proven handlers

- `0x63` dispatches to RVA `0xB7550`. It indexes the actor table at
  VA `0x10480AF0` using the event owner index at event `+2`. A missing
  actor advances three bytes. A nonzero actor byte `+0x1BC` sets the
  event return flag at `+0x25A` and does not advance or replace the action.
  When that byte is zero, work operand `+1` is written to actor word
  `+0x1C0`, actor byte `+0x1BC` becomes `9`, and the program advances three.
- `0x6E` dispatches to RVA `0xB7C40`. Lookup operand `+1` resolves the
  actor; failed lookup, a null actor, or cleared actor `+0x120` bit 9
  advances seven bytes. Otherwise nonzero actor `+0x1BC` parks with the
  return flag. When idle, work operand `+5` is split into low-byte emote
  and arithmetic-shifted high-byte argument, and passed with the actor
  index and trailing arguments `0, 1` to RVA `0xA0B60`. It advances seven.
- `0x99` dispatches to RVA `0xB7CE0`. The same lookup and eligibility
  checks advance five bytes when unavailable. Eligible actor `+0x1BC`
  nonzero parks; zero advances five. The predicate is the actor action
  byte, not a routine-name-specific or emote-only timer.

## Implementation consequence and limits

Unconditionally emitting an emote and advancing `0x63`/`0x6E` can replace
an actor's running action where retail waits. Waiting only on a synthetic
emote key for `0x99` does not represent the predicate above. A fixed Hume
routine duration also does not establish completion for another actor's
race or action state.

Restore the VM's pre-extraction skip behavior for these unsupported
handlers until actor availability, eligibility and current-action feedback
can be represented. This record does not establish emote-file selection,
race-uniform lengths, or a complete actor action-state bridge. Existing
server-packet emote rendering is outside this correction's scope.
