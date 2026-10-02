# Scheduled level-up emission requires positive time

Build: retail-2026-09, patch 30260904_1, FFXiMain.dll SHA-256
f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4.
Addresses below are RVAs relative to image base 0x10000000. The unpacked
.text SHA-256 is b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9.

Generator clone 0x536C0 invokes activation 0x531D0 at 0x537CC. Activation
clears EBX, then writes zero to emission countdown +0xCC at 0x5324D and age
+0xC8 at 0x53253. Ordinary Idle emission subtracts its delta from countdown
at 0x4A67C, stores it at 0x4A682, and compares against 0.0 at 0x4A688.
The x87 comparison result tested at 0x4A690 skips emission for zero/positive
countdown at 0x4A693; only a negative countdown reaches the emission loop.
The PE constant at RVA 0x3295D8 is independently read as float 0.0.

Thus an activated generator's zero countdown remains zero during a zero-time
update. It first emits after a positive delta makes the countdown negative.
This is a computation observed in the retail binary, not an inference from
another client. The main routine's timed g000 generator has a zero authored
duration; the scheduler substitutes one engine unit at 0x59B7C. The DAT mapping and original visual symptom are recorded separately in
2026-10-01-level-up-scheduled-emission.md.

Kuluu's scheduled zero-window non-singleton branch instead compared only
age_frames <= frames. Two consecutive zero-time calls to the production
advance_generator emitted two batches; the regression guard failed with
actual count 2 versus expected 0. The narrow correction requires a positive
delta before that branch emits. Its guard drives zero/zero/positive/zero/
positive updates, checks exactly one total batch, and verifies that a paused
update preserves the live particle's age. Singleton, periodic and auto-run
branches are unchanged. This fixture establishes the computation and paused
edge case; it is not a retail presentation-latency measurement.
