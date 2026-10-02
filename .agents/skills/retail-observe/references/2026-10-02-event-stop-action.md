# Event stop-action operands (retail-2026-09)

Observed 2026-10-02 by inspecting the primary retail client, patch
`30260904_1`. FFXiMain.dll SHA-256:
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Freshly unpacked `.text` SHA-256:
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Image base is `0x10000000`; the dump starts at RVA `0x1000`.

The event dispatcher table at RVA `0xBC960` resolves opcode `0x5E` through
RVA `0xBC57F` to handler `0xB71E0`, and opcode `0x6B` through `0xBC5E7`
to `0xB7070`. Both call helper RVA `0x87D40` before evaluating the operand
at byte 1 with accessor RVA `0xAFAC0`. The helper reads the actor's current
resource/action pair at actor offsets `0x1EC` and `0x1F4`. When both exist
and the render-state guard permits it, it stops that current pair through
the skeleton virtual call at vtable offset `0x29C`. It receives no opcode
operand to use as a routine-name filter.

The handlers then write the operand to actor offset `0x1A4`, call RVA
`0x8DA00`, and write it to skeleton offset `0x7C8`, resetting the skeleton's
other selection at `0x7D8` to spaces. Helper `0x8DA00` updates default idle
selection, including variant handling. Thus the operand supplies the
replacement default motion; it does not select the routine to stop.
Opcode `0x6B` additionally resolves its explicit actor operand at byte 5.

Kuluu keeps the published cue field named `key` for compatibility, but treats
it as replacement idle motion. An internal scheduler-instance identity ties
the current pose to its originating routine so stopping it preserves unrelated
queued effects, including routines with equal names. The event idle override
is scoped to the cutscene and cleared at event end, zoning and disconnect.
This ownership representation is Kuluu machinery, not a claim about retail
object layout. Binary evidence establishes the operand and stop target;
visible interpolation and full authored-scene behavior require runtime proof.
