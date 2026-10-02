# Authored zero-in motion handoff (retail-2026-09)

Inspected 2026-10-02 using the registered retail-2026-09 install, patch
`30260904_1`. Its FFXiMain.dll SHA-256 is
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
The inspected Hume skeleton resource is file ID 7072, `ROM/27/82.DAT`,
SHA-256 `876ac423db19a85fa2bbf8ca41f53bad09fb6a3c064f864572998cd33f91674f`.

`ffxi-dat`'s scheduler dump reads these fields from the authored `sswh`
routine; transition values below are the raw DAT values:

| Routine frame | Motion | Duration | Loops | Transition in | Transition out |
| --- | --- | --- | --- | --- | --- |
| 0 | `mw1?` | 60 | 1 | 10 | 0 |
| 60 | `mw2?` | 60 | 2 | 0 | 20 |

The routine ends at frame 180. The second motion explicitly requests no
incoming transition. These data establish that instruction; they do not
establish the complete retail scheduler's clock or interpolation algorithm.

Kuluu's production capture selected `mw2?` at simulation frame 180, but the
rendered arm lowered at frames 186 and 192 before returning raised at 198.
`SkeletonAnimator::set_next_animation` assigned the zero-in replacement while
retaining the previous transition. `get_joint_transform` continued sampling
that obsolete transition until it expired. Clearing the transition on immediate
replacement makes rendered joint selection agree with the authored instruction.
The regression is tracked in [#894](https://github.com/jondwillis/kuluu-ffxi/issues/894).
