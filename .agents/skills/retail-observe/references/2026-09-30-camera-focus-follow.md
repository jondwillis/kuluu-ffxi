# Chase camera focus follow

Build: retail-2026-09, FFXiMain.dll SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Unpacked .text SHA-256: `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.

The following-camera routine starts at RVA 0x1EE60. The focus loop at 0x1F5A2
obtains CheckTick, converts it to an integer and repeats:
focus += (actor_anchor - focus) * 0.25. The immediate 0x3E800000 is pushed at
0x1F5D3. Independently inspected callees: subtract 0x27120, in-place scale
0x272B0, in-place add 0x26F20. The result reaches the look-at setter at 0x1F602.
There is no spatial dead zone in this loop. Actor state controls the vertical
anchor and whether it updates; this finding concerns ordinary moving actors.

CheckTick at RVA 0x14CF0 reads manager +0x28, with minimum 1. The timer writer
at 0x12B9F divides 60.0 (constant RVA 0x329CE8) by effective frame rate and
stores that scale. These are 60 Hz ticks. Kuluu uses 1 - 0.75^(60*dt): it
matches the recurrence at whole ticks and interpolates fractional ticks to
avoid render-rate-dependent steps.

PR803 froze focus within 0.5 yalms, citing a camera tutorial. That suppresses
response to small lateral motion. Replacing the threshold addresses the reported
hesitation. Eye-distance, collision, lock-on, vertical anchor and projection
are separate policies; this finding does not establish complete camera parity.
No live retail comparison was performed.

Local disassembly: /private/tmp/pr804-camera-follow-disasm.txt,
/private/tmp/pr804-camera-vector-add.txt, /private/tmp/pr804-camera-vector-sub.txt,
/private/tmp/pr804-camera-vector-scale.txt and /private/tmp/pr804-time-candidates.txt.
