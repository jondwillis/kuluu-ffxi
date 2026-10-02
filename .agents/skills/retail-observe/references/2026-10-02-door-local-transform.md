# Door animation matrix composition (retail-2026-09)

Inspected 2026-10-02 from the registered retail install, patch `30260904_1`.
FFXiMain.dll SHA-256:
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Freshly unpacked `.text` SHA-256:
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
The fresh unpack matched the saved dump. Image base is `0x10000000`;
all addresses below are RVAs. This is static binary and DAT inspection,
not a live retail capture.

## Rotation and translation offsets

Scheduler dispatch at `0x57FB0`, table `0x5DC1C`, maps tag `0x0D` to
`0x5A1BE`. That handler passes the stage vector at offset 8 and slot at
`0x14` to task constructor `0x439C0`. The task reads its initial rotation
from actor offset `0x5E4 + slot * 16`; tick `0x43B00` interpolates to the
stage target and calls `0xACDB0`. This setter stores the rotation offset
and calls `0xACEF0`. The analogous translation setter `0xACEA0` stores
its vector at actor offset `0x624 + slot * 16` and calls the same helper.

`0xACEF0` constructs a rotation matrix from the offset vector, writes the
translation offsets to matrix elements at `0x30`, `0x34`, `0x38`, and
passes this matrix to `0xACF90`. The latter copies the animated matrix,
then calls `0x27D10` with the stored initial leaf matrix as its argument.

`0x27D10` multiplies `this * argument` in row-major, row-vector notation.
For example, result element 0 is `this[0]*arg[0] + this[1]*arg[4] +
this[2]*arg[8] + this[3]*arg[12]`; successive columns and rows follow
that ordering. Therefore the composition is `animated * initial` in
retail notation, or `initial * animated` in Bevy's column-vector notation.
It does not add animation Euler angles to placement Euler angles.

The initial matrices are captured by `0xACBA0`: it initializes actor
`0x668` to the inline array at `0x66C`, then copies each subchunk matrix
through `0xAD300` / `0x171030`. The final composed matrix goes through
`0xACFF0` to `0x171130`, which copies all 16 elements into the rendered
subchunk at offset `0x40`. This preserves the placement's mirrored scale
in the matrix multiplication rather than applying it after a combined
Euler rotation.

## DAT cross-check and scope

Southern San d'Oria, DAT file ID 330 (`ROM/1/31.DAT`), SHA-256
`4d17ce938e53da7a075d985aba5902723527884b6a86fd2046e90ea232e91433`,
has paired `_6ey` leaves with identical approximately +80-degree Y
rotation stages and opposite Z scale signs. `_6ek` uses opposite X scale
signs; `_6e7` uses approximately -80-degree stages. Adding Euler angles
before scale makes the two free edges travel through opposite sides of
the doorway in these fixtures. Local matrix composition lets their
mirrored placements determine their different world-space rotations.

The user observed same-direction paired swing in retail. This record
establishes the native composition rule, not a universal claim that every
two-member model group must swing in the same direction. It does not
establish door event-exit policy, timing, or platform-height overrides.

## Renderer representation cross-check

Scanning paired rotating groups in the registered install produced 575
same-through-direction results after native matrix composition. Two further
pairs (`_jke`, `_jkd`, DAT 156) rotate about other axes and have effectively
zero horizontal displacement; the sign of floating-point noise is not
opposite-swing evidence.

Sampling each decoded rotation stage at quarter intervals found six groups
whose composed matrix loses information when flattened into one TRS:
DAT 187 `_2f0`, DAT 269 `_4pc`, and DAT 337 `_6l2`, `_6l3`, `_6l4`, `_6l6`.
For example `_2f0` has absolute scale `[0.89, 0.82, 1]`, and `_4pc`
`[1, 0.825, 0.774]`. Preserving an authored-placement parent and a local
animation mesh child retains the full affine under Bevy propagation.
The existing world-height override is separate from the native animation
offsets; its target world Y must be converted back into the local frame.
