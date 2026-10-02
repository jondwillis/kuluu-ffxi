# Level-up sound mixing boundary, 2026-10-02

Input is clean `retail`, client `retail-2026-09`, patch `30260904_1`.
FFXiMain.dll SHA-256 is
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`;
unpacked text SHA-256 is
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Image base is VA `0x10000000`; text begins at RVA `0x1000`.

## Native route

After opcode `0x4A`'s local-actor gate, the common sound path at RVA
`0x5B794..0x5B7BD` passes volume `0x7F`, pan range 64, stage distance
fields +`0x14/+0x18`, and the actor position obtained through virtual
+`0x1BC` to constructor RVA `0x370D0`. The sound element stores position
at +`0x19C/+0x1A0`, distance fields +`0x1B0/+0x1B4/+0x1B8`, pan range
+`0x1D0` and volume +`0x1BE`. Constructor RVA `0x369F0` initializes its
mode byte +`0x1E9` to zero.

Sound update RVA `0x36540..0x36615` feeds these fields to the positional
calculation at RVA `0x36C70`. Default mode weights the camera-to-emitter
Y component by 3 before calculating distance. Near/far distances select
full gain, linear falloff or silence; integer conversion truncates.
Zero distance parameters select globals at VA `0x103516EC/0x103516E8`.
A zero width selects VA `0x103516F0`; explicit width is multiplied by
512. Behind-camera sound receives a further factor from VA `0x103516E4`.
Camera-space X, depth and focal length determine a clamped pan ratio;
the result is centered at 64 and bounded to `0..127`. Update applies the
global effect-volume multiplier at RVA `0x36625..0x3662F` before playback.

## Confirmed implementation boundary

The focused correction establishes the authored SE7 asset and per-routine
local-player predicate. It does not reproduce this entire positional mixer.
Kuluu's current opcode `0x4A` dispatch emits a dry `SfxEvent` and its shared
mixer lacks native pan. The exact initialized globals, matrix conventions,
and effective gain under a live camera remain unverified; substituting
plausible defaults would not establish retail parity.

A decoded SPW mux proves the chosen authored cue, not actual output from
Bevy/rodio. Runtime event dispatch and output capture are separate evidence.
The broader mixer task remains open independently of the bounded receiver
and particle corrections.
