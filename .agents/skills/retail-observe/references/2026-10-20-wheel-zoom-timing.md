# Wheel zoom timing

## Scope

This reader-only investigation covers `retail-2026-09`.
It verifies the wheel input scale, the focal change per eligible camera update, and the default frame timing.
It does not establish a complete end-to-end notch displacement measurement.
No original-client input or implementation change occurred during this investigation.

## Inputs and units

One standard Windows wheel notch has a signed delta of 120 wheel units.
The normal wheel route divides that delta by 120 with truncation toward zero.
The resulting signed notch count controls the duration of wheel zoom.
Each notch adds two units of duration.
Each active client frame reduces the remaining duration by one toward zero.
Opposite wheel directions can cancel remaining duration.

This route does not define a conversion from touchpad pixel deltas to wheel notches.
It does not accumulate a sub-notch Windows delta across separate messages in the inspected normal message route.
A separate input route supplies signed wheel counts directly; its device mapping remains unverified.

## Focal response

The focal value controls the perspective zoom; it is not the camera boom distance.
For each eligible camera update, let `t` be the current frame duration in units of one sixtieth of a second.
Positive remaining wheel duration increases the focal value by `6 * t`.
Negative remaining duration decreases it by `6 * t`.
The supported focal band is 242 through 900.
Reaching a band boundary clears remaining wheel duration.

The two keyboard zoom directions take priority over wheel zoom.
A single keyboard zoom direction uses the same `6 * t` change and clears remaining wheel duration.
Both keyboard zoom directions invoke a separate neutral-reset behavior.
The wheel branch also requires an eligible mouse/control mode.
The complete mapping from this predicate to user-facing settings remains outside this record.

## Default timing and conditional totals

The client starts with a minimum frame duration of two sixtieths of a second.
This corresponds to the default 30 frames per second.
The timing update derives a frame duration from the timer and applies smoothing, integer conversion, and bounds.
The camera timing getter returns at least one sixtieth of a second.

At a steady default rate, one eligible camera update changes focal value by 12 units.
At a steady 60 updates per second, one eligible update changes it by 6 units.
If one notch reaches two eligible camera updates, the totals are 24 and 12 focal units, respectively.
Those conditional totals last about 66.7 and 33.3 milliseconds, respectively.

The investigation did not settle the full order of wheel arrival, camera consumption, and duration reduction across a frame.
The camera consumer reaches actor callbacks through indirect dispatch.
The selected callback and its scheduler position were not resolved for the eligible gameplay mode.
This missing connection prevents a complete ordering proof.
Therefore, two eligible camera updates per notch are not yet an independently verified end-to-end result.
Do not promote the conditional 24-unit total into a verified notch specification.
The per-update scale and the two distinct time units are verified.

## Observable validation still required

Use an eligible third-person view with neither keyboard zoom direction active.
Start with a focal value away from either band boundary.
Deliver one verified wheel notch.
Measure the perspective change and its duration at the default frame rate.
Repeat at 60 frames per second if a supported configuration permits it.
A fixed scene can distinguish a duration change from a focal change.

Earlier Wine helper tests submitted one wheel step in each direction without a visible zoom change.
Those tests do not prove delivery to the camera zoom channel.
They cannot validate or refute the conditional totals above.

A later test sent one standard positive wheel message directly to the unique foreground original-client window.
The window acknowledged the message within the bounded timeout.
Fresh captures showed no visible camera change, and the test stopped after that one message.
Acknowledgement does not establish delivery to an eligible camera zoom channel.
This test used the HXI/Wine stack, not the build inspected above.
Its proof is `artifacts/verify/release-080-current-main/retail-reference/wheel-message-proof.json`.

## Provenance

Build: `retail-2026-09`, patch `30260904_1`.
Installed DLL SHA-256: `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Unpacked text SHA-256: `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Image base VA: `0x10000000`; text begins at RVA `0x1000`.

The installed file is the registered retail install's `FFXiMain.dll`.
The hash-matched dump was `/Users/jon/src/ffxi/artifacts/polre/unpacked/FFXiMain-f2245d1c.text.bin`.
Capstone decoded the bounded paths; the DLL and runtime were not changed.
Saved disassembly is under `artifacts/verify/release-080-current-main/retail-reference/wheel-reader/`.

Normal wheel message normalization is at RVA `0x1d90` through `0x1df7`.
The signed multiply and shifts implement division by 120 before the call at RVA `0x1dec`.
The alternate direct-count caller is at RVA `0x1245ff` through `0x12461f`.
The duration accumulator at RVA `0x25e210` adds twice its signed argument to VA `0x1067a298`.
The reader is RVA `0x25e200`; clear is RVA `0x25e230`.
The reduction function is RVA `0x25e240`, reached through thunk RVA `0x25e0e0` from RVA `0x1295a`.

The camera consumer is RVA `0x1f7de` through `0x1f968`.
The eligibility helper is RVA `0x25e050`.
The focal getter/setter are RVAs `0x152c0` and `0x15290`.
Read-only scalar values at RVAs `0x32a3e8`, `0x32a3d8`, and `0x32a3d4` are 6, 900, and 242.
Camera-update entry RVA `0x1ee60` has actor callers at RVAs `0xa5eac` and `0xa66d3`.
Their complete frame-order relation to the reduction call was not traced.

The frame object constructor at RVA `0x10795` initializes its minimum-duration field at offset `0x30` to 2.
The frame-duration producer at RVAs `0x12b12` through `0x12ca6` uses scalar 60 at RVA `0x329ce8`.
It stores the camera tick at object offset `0x28` after smoothing and integer conversion.
The minimum-duration field bounds that tick from below; scalar 20 bounds it from above.
The camera tick getter at RVA `0x14cf0` returns at least scalar 1.
The timer callback implementations were not traced, so unusual timer modes remain outside the claim.

The production label `M34` supplied search locations only.
No dated record with that label established an independent end-to-end wheel measurement.

Follow-up ordering trace: direct camera consumption reaches the actor path at RVA `0xc81f0`, plus a second actor path near RVA `0x8d2b5`.
The former belongs to a larger actor update and uses virtual eligibility checks before calling the camera helper.
The main frame path contains the duration reduction at RVA `0x1295a` and several earlier indirect subsystem calls.
The actor update dispatch slot and its active scheduler registration were not resolved.
Therefore, matching a nearby direct call cannot prove which camera consumer runs before or after reduction.
No end-to-end ordering claim was added from this incomplete trace.
