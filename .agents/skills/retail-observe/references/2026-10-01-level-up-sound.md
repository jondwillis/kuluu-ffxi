# Level-up sound, 2026-10-01

## Inputs

Clean registry install `retail`, KNOWN_CLIENTS `retail-2026-09`, patch
`30260904_1`. FFXiMain.dll SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Image base VA `0x10000000`; unpacked `.text` starts at RVA `0x1000`,
SHA-256 `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Analysis used the existing bounded POL1 unpack and ephemeral Capstone.

## Authored sound and trigger

DAT file ID 3310 resolves to `ROM/13/35.DAT`. The `main` routine has an
opcode `0x4A` sound stage at frame zero, naming `0007`. That type-`0x3D`
resource has `SeSep` header and FileID 7 at body offset 8. The other SEP,
`2094`, has FileID 12094 and is not the frame-zero stage's named resource.
Read the resource ID from the DAT rather than assigning a system event ID.

SE 7 resolves to `sound/win/se/se000/se000007.spw`. The existing
`ffxi-audio` decoder produced 277248 mono frames at 48000 Hz, 5.776 seconds,
with no loop marker. This establishes the authored asset, not a live retail
comparison of its decoded audio quality or mixing.

## Retail opcode dispatch

Scheduler opcode dispatch at RVA `0x57FB0` reads the opcode from task tag
pointer `+0x88`, subtracts two, and indexes the table at RVA `0x5DC1C`.
Opcode `0x4A` selects RVA `0x5B5DA`. It obtains the target through
RVA `0x57BF0`, calls actor vtable slot `+0x304`, and skips playback if false.
For the skeleton actor vtable at RVA `0x330F40`, this slot is RVA `0xA4640`:
it looks up the control actor through VA `0x1047D600` and compares the
returned actor pointer with the receiver. This is the local-player gate.

On success, `0x4A` joins the regular sound resource path at RVA `0x5B61C`.
The code resolves the four-character resource at tag offset 8 as type
`0x3D`, then calls playback at RVA `0x370D0`. Opcode `0x60` enters a separate
path at RVA `0x5B7F0`, without the local-player predicate. The two opcodes
must not share a global audibility policy.

The existing renderer's non-positional mixing policy was not established
by this trace. Its exact volume/pan/pitch law still needs investigation;
this change preserves that policy and corrects only the demonstrated gate.

## Kuluu verification boundaries

The production scheduler already resolves this authored SEP and emits an
SfxEvent. The original level-up demo disabled Bevy audio and omitted the
sound stage consumer, so its silence did not establish a missing production
asset. Wire `dispatch_sound_stages` and `play_sfx_system` into the demo,
using the same decoded source as the client. Mark the demonstrated owner
as the local player so the retail gate applies.

A synthetic dispatch test distinguishes a local target, another target even
when the caster is local, and a global sound. The real-install guard resolves
the frame-zero SEP and decodes nonzero installed audio. Both passed against
the clean retail install. None of this proves server-trigger delivery or the
full visual/mixing parity with a live retail client.

## Repeatable demonstration

The demo advances visible playback from elapsed time; offscreen capture retains
a deterministic 60 Hz clock and records every frame. A clean-retail run emitted
SE 7 through the production scheduler and playback systems and captured 361
frames from effect frame 0 through 360. The capture spans six seconds after
effect start to retain the 5.776-second sound. Frame 100 was inspected and
shows upright lettering. A local review clip muxes those frames with the
authored SPW decoded by `ffxi-audio`; it is not an OS loopback recording or
a live retail comparison. Visible replay does not depend on display refresh.

The demo has no session transitions: the effect owner is despawned on replay,
one-shot audio uses production DESPAWN playback, and its decoded-audio cache
lives until process exit.
