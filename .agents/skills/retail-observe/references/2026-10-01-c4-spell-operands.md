# C4 spell operands

Observed by binary inspection on 2026-10-01. Build: `retail-2026-09`,
patch `30260904_1`, FFXiMain.dll SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Unpacked `.text` SHA-256:
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`;
its first byte corresponds to VA `0x10001000`, RVA `0x1000`.
All addresses below are RVAs relative to image base `0x10000000`.

## Dispatch and layout

ExecProg at `0xBC280` uses the table at `0xBC960`. Opcode 0x73 routes
through `0xBC627` to `0xB4540`, which passes shift 0 to helper `0xB4590`.
Opcode 0xC4 routes through `0xBC8AF` to `0xB4550`; cases 0, 1 and 2 pass
mode 0x10, 0x11 and 0x12 respectively, with shift 1, to the same helper.

The helper loads that shift into EBX at `0xB4598`. Its first actor operand
uses EBX+3 at `0xB45A6`, then the event operand accessor `0xAFAC0` and actor
resolver `0xAFB00`. The second actor uses EBX+7 at `0xB45CB`.
The work operand uses EBX+1 at `0xB4672`, calling `0xAF4B0`.
The completion path adds EBX+11 at `0xB46A2` before updating the execution
pointer; the other supported mode paths use the same width.

Thus C4's case byte is at +1, two-byte work operand at +2, first four-byte
actor operand at +4, second at +8, and total width is 12 bytes. The work
operand and first actor do not overlap. The older XiEvents 2022 description
omits the shift from its first-actor expression; that description is not the
authority for this build.

## Limits

For cases above 2, the outer C4 handler branches at `0xB456F` to `0xB458E`
and returns without advancing. Kuluu currently skips the 12-byte instruction
without emitting a cue for unsupported cases. That is a robustness policy,
not measured retail parity, and is unchanged by this operand correction.
This finding establishes operand layout and dispatch, not visible spell
timing or complete semantics of each mode.

Local disassembly retained at
`/private/tmp/kuluu-pr811-magic-retail-disasm.txt`. The regression constructs
valid non-overlapping instructions for all three supported cases; offset +3
produces the wrong caster and fails the guard.
