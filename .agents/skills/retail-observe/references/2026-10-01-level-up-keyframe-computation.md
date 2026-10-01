# Level-up keyframe computation, 2026-10-01

Question: does the level-up lettering require a special eased curve or shimmer?

## Inputs and identification

Primary client: `retail-2026-09`, patch `30260904_1`, named registry install
`retail` (not the texture-modified `ashenbubs` install). `FFXiMain.dll` SHA-256:
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Image base is `0x10000000`; decompressed `.text` starts at RVA `0x1000`.
The dump SHA-256 is
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
All addresses below are VAs for this build; subtract the image base for RVAs.
The registered installation resolves file 3310 to `ROM/13/35.DAT`.

XIClient `Resource/Derived/CMoKeyframe.cpp` and
`World/Generator/CYyGenerator.cpp` supplied search hypotheses. The arithmetic,
branches and callers below were independently checked in the retail dump with
Capstone, rather than accepted from those reconstructions.

## Verified computation

- Generator element updater begins at VA `0x1004A980`. Its opcode dispatch at
  `0x1004A9F3` uses the jump table at `0x1004E03C`, indexed by opcode minus one.
- Opcode `0x0E`, VA `0x1004D6BD`, obtains the initial life through
  `0x1004E440` (element offset `0x114`) and remaining/initial life through
  `0x1004E450` (offset `0x110` divided by `0x114`). Track progress is
  `1 - remaining / initial`, with zero when initial life is zero.
- Opcode `0x15` (scale X), VA `0x1004BB10`, reads the track binding from the
  element allocation slot selected by `(header >> 13) & 0x3F`. It tests the
  low nibble of the binding flags. Zero calls VA `0x100544D0`, which temporarily
  substitutes the initial channel value into the first key and calls the linear
  sampler at `0x10054500`. Nonzero calls VA `0x100546F0`, the corresponding
  initial-value override for the eased sampler.
- Linear sampler `0x10054500` walks adjacent `(time, value)` float pairs and
  computes `v0 + (t-t0)/(t1-t0) * (v1-v0)`. The eased sampler at `0x100546F0`
  and no-override variant `0x100547A0` use an ease-in/out quadratic weight:
  `2*u*u` below the segment midpoint, `1 - 2*(1-u)*(1-u)` above it.
  Existence of that sampler does not establish that level-up uses it.
- Opcode `0x1B` (alpha), VA `0x1004BD50`, invokes binding helper
  `0x1004E4E0`. The helper makes the same low-nibble choice, including the
  initial-value override. The result is multiplied by 255 and negative values
  are clamped before conversion to the element alpha byte.
- Opcode `0x02` (position integration), VA `0x1004D74B`, scales velocity by
  the renderer delta accessor `0x1004E3D0` (manager offset `0xEB0`) before
  adding it to position. Opcode `0x2C`, VA `0x1004C6CB`, feeds the authored
  damping and that delta into the power routine at `0x10312C90` before scaling
  the velocity. This is exponential damping, not a constant linear slowdown.
  The caller chain that produces the renderer delta's units was not traced.

## Actual level-up tracks

Initializer blocks store the resource reference first, the four-byte resource
ID next, then the binding flags. In the clean DAT, those flags are zero for:

- `g001` and `g002`: scale-X initializer `0x27`, resource `k001`.
- `g000`: alpha initializer `0x2D`, resource `k000`.
- `g004`: alpha initializer `0x2D`, resource `k002`.

Thus these level-up tracks select the linear segment sampler, not the eased
sampler. `k001` is `(0,0), (0.4453125,0.06999997794628143), (1,0)`.
`k000` is `(0,0), (0.1750001162290573,1),
(0.3338544964790344,0.501960813999176),
(0.505729615688324,0.501960813999176), (1,-0.018039260059595108)`.
`k002` is `(0,0), (1,0)`; the initial-value substitution matters for its first
segment. These piecewise curves already create distinct rises, holds and fades.
`g000` also carries opcode `0x2C` damping with the authored float
`0.924001...` (raw little-endian bytes `54 8b 6c 3f`).

## Limits and next comparison

This confirms authored nonuniform scale/alpha and exponential motion. It does
not prove a separate shimmer algorithm, a frame-rate policy, or runtime visual
parity. Level-up also uses layered sprite geometry and child generators; a
static frame or low-frame-rate video cannot isolate those from sampling or
presentation artifacts. Do not add a generic shimmer or ease every track based
on memory of the retail effect. Capture clean retail and Kuluu at matched camera,
install, time scale and recording frame rate before judging the remaining
appearance, and trace the renderer delta producer if timing is disputed.
