# Critical spark condition grammar, 2026-10-04

## Authored rule

The clean global `crtl` routine has an outer condition expression consisting
of result selector `0x2B`, constant operand 2, and operator `0x11` (the mask
test established in the earlier melee dispatch observation). The constant is
an operand of that operator; it is not an instruction to compare the entire
result value for equality with 2.

The outer predicate is true when `(result_info & 2) != 0`. Other information
bits do not clear the critical gate. In particular, adding `Defeated` to a
critical result leaves the gate true.

Inside that true arm, the routine tests selector `0x38` directly as a truth
value. That expression has no additional constant operand or comparison
operator. Its true branch starts `hi14`; its else branch starts `hi29`.
There is no random selection and no `ldam` call in this routine.

## Required outcomes

| Result information | Value | Outer critical predicate | Attacker ServerID high byte clear | Attacker ServerID high byte set |
| --- | ---: | --- | --- | --- |
| No flags | 0 | False | Neither spark branch | Neither spark branch |
| CriticalHit | 2 | True | `hi14` | `hi29` |
| Defeated | 1 | False | Neither spark branch | Neither spark branch |
| CriticalHit and Defeated | 3 | True | `hi14` | `hi29` |

The attacker classifier is exactly whether
`(attacker_server_id & 0xFF000000) == 0`. In the conventional player/NPC
identity partition, player IDs select `hi14` and high-byte-set NPC IDs select
`hi29`, provided the critical gate is true. The rule is stated by identity
bits so an unusual server assignment remains unambiguous. It does not test
the victim's class, the attacker's model or race, or selector `0x3B`.

The selected subroutine retains attacker-as-caster and victim-as-target,
as established by the earlier melee dispatch record.

## Evidence limits

This supplement independently rereads the authored DAT expression and reuses
the earlier verified client mappings for selectors and mask-test behavior.
No fresh binary inspection, live capture, or production evaluator execution
was performed. It establishes the branch-selection rule, not whether either
spark is currently visible in Kuluu or its full rendering behavior.

## Provenance

Freshly read clean retail DAT 0:
`/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/ROM/0/0.DAT`.
SHA-256 `5b2ac1bc3efbb73a3c9ffbb884ed6b7066355910a91635bfeef14bd027c6c158`.
The `crtl` chunk starts at file offset `0x80070`. An independent bounded
stage walk from `0x800AC` through the terminating stage ending at `0x80170`
asserted the complete expression sequence:

- Field operand words `0x0003001C, 0x2B`.
- Constant operand words `0x0001001C, 2`.
- Operator word `0x11`.
- Nested field operand words `0x0003001C, 0x38`.

It also asserted the two same-context routine names in order, `hi14`, `hi29`.
The trusted prebuilt `target/debug/examples/dat-routine-stages` independently
reported the nested condition/control stages, `hi14` then else `hi29`, all at
authored delay 1.

Client-runtime mappings are reused from the independently inspected records
`2026-10-04-melee-damage-dispatch.md` and
`2026-10-04-melee-recoil-and-result-fields.md`, prepared in this same evidence
session. Those records pin `retail-2026-09`, DLL SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`, and
`horizonxi-2023`, DLL SHA-256
`f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c`.
They establish selector `0x2B` as five-bit result information and selector
`0x38` as the callback attacker's ServerID high-byte-clear classifier.
This supplement did not reopen those binaries or rehash their DLLs.

Wire flag names come from `vendor/server/src/map/enums/action/info.h`:
`Defeated = 1`, `CriticalHit = 2`. The LSB dynamic non-player ID allocator in
`vendor/server/src/map/zone_entities.cpp` sets `0x01000000` in those IDs.
The outcome table is the direct logical consequence of the recorded mask-2
predicate and identity classifier; it is not a claimed live capture.

Inspection stopped when this supplement was prepared. Production source,
the shared index, and existing records were not edited.
