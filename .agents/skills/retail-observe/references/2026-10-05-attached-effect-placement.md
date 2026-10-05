# Attached effect placement, 2026-10-05

Where the client places, orients and scales a particle generator whose attach
word binds it to an actor, established by static inspection of the installed
`retail-2026-09` client (patch `30260904_1`) and its DATs, and worked through
for the level-up effect, file 3310 = `ROM/13/35.DAT`, on the Hume M skeleton
(file 7072). Covered: how the attach word becomes a frame for every mode the
data can carry, how a reference index becomes a world point, whether that point
tracks the pose and the actor's scale, the opt-in actor-fit scaling, the
per-update follow, and how an element and its billboard are composed from the
frame. Not covered: emission counts, keyframe sampling and the sound stage
(earlier records), and the live on-screen result, which no retail capture on
this machine shows.

## Inputs

Generator chunk (type 0x05) fields, body offsets excluding the 16-byte chunk
header:

- The attach word: the u16 `attachFlags` as the low half and the u16
  `additionalAttachFlags` as the high half. Mode = bits 0-3 with bit 16 as a
  fifth bit (0-31). Source reference = bits 4-9 with bit 18 as a seventh bit
  (0-127). Target reference = bits 10-15 (0-63). Position-fit nibble = bits
  20-23. Model-fit nibble = bits 24-27. The level-up generators carry
  `0x0011` / `0x0000`: mode 1, source reference 1, target reference 0, both fit
  nibbles 0.
- Section 1 (generator updaters): opcode 0x11 is the follow; its payload word
  carries the follow flags and rate. Opcodes 0x0E, 0x0F and 0x10 also rebuild
  the frame (see below).
- Section 2 setup block 0x01: payload word 0 is the setup flags (bit 23 gives
  the element a private copy of the frame at birth), payload words 4-6 are the
  element's starting position, payload word 7 carries the particle life.
- Section 2 block 0x7B (optional): two offset vectors, payload words 1-3 for
  the source-reference point and 4-6 for the target-reference point. Only the
  two-point modes read it. The level-up generators carry none; in a 630-file
  sample 20 of 211 two-point generators do (typically a vertical raise of the
  target-side point).
- Position initializers (0x02 velocity, 0x1F spherical spread, 0x45 parent
  position copy), scale 0x0F and its tracks, Euler rotation 0x09.

Model data per actor: the skeleton's reference table (128 entries, each a joint
index, a rotation and a translation in that joint's frame), the actor position,
its facing (yaw), its scale set (an override scale, three per-axis scales and a
uniform scale, each negative when unset; the effective scale for an axis is the
override when set, else that axis's scale when set, else the uniform one), a
model flag set once the pose has been evaluated, the posed world matrix of
every joint, and for the fit scale the skeleton's first bounding box.

## Attach frame

At activation the client builds one matrix, the frame, for the generator from
the attach word; every element of that generator expresses its position in
that frame. `point(i, A, B)` below is the reference-point rule of the next
section evaluated on actor A with B as the other actor. The caster is the
action's source actor, the target its target actor.

| Mode | Frame |
| --- | --- |
| 0 | none: the generator is unattached, elements are in world space, no fit scale |
| 1 | translation `point(source ref, caster, target)`, rotation = caster yaw |
| 2 | translation `point(target ref, target, caster)`, rotation = target yaw |
| 4 | translation `point(target ref, target, caster)`, rotation = caster yaw |
| 5 | translation `point(source ref, caster, target)`, rotation = target yaw |
| 3 / 6 | both points; X axis from the caster point to the target point (3) or the reverse (6), pitch and yaw from that direction, translation at the start point; the point distance replaces the X position-fit scale |
| 16 / 17 | the same two-point basis with both points on the caster (16) or the target (17), distance override |
| 13 / 18 | the basis of 3 (13) or of 6 (18) without the distance override |
| 22-27 | modes 3, 4, 5, 6, 13 and 18 in that order, with an alternate actor resolved from the caster substituted for the target when one exists |
| 7 / 8 | the current view orientation, translated to the caster (7) or target (8) point |
| 9 / 10 | the full posed locator matrix (rotation and translation) of the caster's source reference (9) or the target's target reference (10); references 54 and 55 remap to the weapon locators 126 and 127, 56-60 to 102-106 |
| 11 / 12 | the heading (yaw) of that posed locator matrix only, translated to the locator point |
| 14 / 15 | identity translated to a zone-supplied point rotated by the client's static attach matrix (the sun / moon placement) |
| 19 | identity translated to the target's reference 2 point moved 0.18 toward the ground |
| 20 / 21 | the client's static attach matrix (21 with the translation cleared) |
| other | identity |

A mode whose required actor is missing yields identity (13 and 18 need only
one of the two). Modes 2, 4, 8, 10, 12 and 23 read bits 10-15; modes 1, 5, 7,
9, 11 and 24 read bits 4-9 plus bit 18; the two-point modes 3, 6, 13, 16, 17,
18, 22, 25, 26 and 27 read both. The attach word is therefore two indices,
not one. In the two-point modes each point is first moved by its authored
offset from the generator's 0x7B block (words 1-3 for the source-reference
point, 4-6 for the target-reference point), rotated into the yaw of the actor
the point is evaluated on; a generator without that block adds nothing, and
the single-point modes never add an offset. The level-up word puts the frame
at `point(1, caster, target)` turned to the caster's yaw.

## Reference point

`point(i, A, B)` for 0 <= i < 48 is actor A's locator accessor, evaluated on
A's model:

- i = 2: A's position plus the table translation of entry 2 multiplied
  component-wise by A's model scale vector (the entry's vertical component by
  the height scale). The entry's joint and rotation are ignored and no pose is
  needed. This is the only special-cased index; it is the nameplate anchor.
- any other i: if A's model has never had its pose evaluated, A's position
  (its feet); otherwise the posed locator: the point the entry's translation
  maps to under the current world matrix of the entry's joint. Those world
  matrices are the ones A's mesh is skinned with, and they are evaluated under
  A's scale, facing and position (the pose routine composes the joints beneath
  a scale x rotation x translation matrix built from the effective per-axis
  scales, the facing and the position, and sets the pose-evaluated flag when it
  succeeds), so the point moves with the animation every frame and scales with
  the actor. The entry's rotation does not move the point; it only orients the
  locator matrix that modes 9-12 use.
- When the entry's joint is the root (joint 0) and one of three riding or
  seated states holds, the point is raised by an offset times A's effective
  height scale. Two of the states use a flat 1.3 or 0.5. The third consults
  the ridden object: 1.3 when A has no ridden object or its seat class is 0 or
  6, a per-race table for two object kinds, otherwise 0.5. A standing actor
  adds nothing.

Special indices, in any mode that resolves a point:

- 48: ground: a collision probe along the vertical through `point(0)` from 1
  unit above to 6 units below; the hit point, else the origin.
- 49 / 50: the ring entry 13-20 of A nearest to B's position. Both indices
  compute the same thing; which actor is A and which is B comes from the mode.
- 51: the ring entry nearest the camera eye.
- 52 / 53: A's last touch-floor / water-surface point.
- 54 -> 126, 55 -> 127, 56-60 -> 102-106, then the locator rule.

For the playable skeletons reference 1 is an ordinary table entry bound to a
mid-body joint with zero rotation and translation, so its point is that joint's
posed position. Bind-pose height above the feet (model Y is down, so the stored
Y is negative), with the joint: Hume M 1.05 (joint 25), Hume F 0.99 (3),
Elvaan M 1.22 (3), Elvaan F 1.13 (3), Tarutaru 0.30 (72), Mithra 0.99 (3),
Galka 1.17 (3). Reference 2 is root plus 2.0 up (Elvaan 2.4, Galka 2.6,
Tarutaru 1.3); 12 is root plus the model height (Hume M 1.81, Hume F and
Mithra 1.68, Elvaan M 2.09, Elvaan F 1.97, Tarutaru 1.00, Galka 2.30); 3 is a
neck joint (Hume M 1.50); 21 is root plus 1.24 up (Hume M); 13-20 ring the root
(Hume M at 1.1 up, radius 0.17-0.32; Galka at 1.2 up, radius 0.5); 49-51 are
filed as root with no offset, which is why they act as selectors. The client
holds no per-race table; every value is read from the skeleton.

## Scale

Nothing in this path multiplies an authored position, velocity or particle
size by the actor's height, nameplate anchor or a race table. The attach point
itself scales with the actor: reference 2 explicitly, every other reference
through the posed joint matrices. Actor-dependent scaling of the particles
themselves is opt-in through the fit nibbles:

- Position-fit nibble (bits 20-23) nonzero: nibble 1-4 selects the caster, 5-8
  the target. With `w` = width scale, `h` = height scale and `f` a
  per-generator factor (1 when unset): nibble low two bits 0 -> uniform
  `(max(w, h) - 1) * f + 1` on all axes; bit 1 set -> Y = `(h - 1) * f + 1`;
  bit 0 set -> X and Z = `(w - 1) * f + 1`; an unselected axis stays 1. The
  result multiplies every element's local position before the frame is
  applied. Modes 3, 6, 16 and 17 then force X to the point distance.
- Model-fit nibble (bits 24-27): the same selection with its own factor; the
  result multiplies the element's scale matrix, i.e. the particle's size.
- Width scale = (box x extent) x s / 1.7; height scale = (box y extent) x s /
  1.9; depth scale = (box z extent) x s / 1.9, where the box is the skeleton's
  first bounding box and `s` is the actor's effective scale for that axis. At
  scale 1 the installed skeletons give width / height: Hume M 0.82 / 1.00,
  Hume F and Mithra 0.94 / 1.05, Elvaan 0.94 / 1.19, Tarutaru 0.47 / 0.55,
  Galka 1.18 / 1.32. An actor without a model gives 1.

## Per-update follow

- The frame is computed once at activation. Generator updater 0x11 rebuilds
  the attach matrix on each generator update and blends the stored frame toward
  it: payload bit 0 blends the translation row, bit 1 the three rotation rows,
  at rate `((word >> 2) & 0xFF) / 255` per update (255 is a snap). The same
  updater marks the generator, and from then on every live element's update
  blends the frame that element reads, whether the shared generator frame or
  its own private birth copy, toward a freshly built attach matrix at the same
  rate. Generator updaters 0x0E, 0x0F and 0x10 rebuild the whole frame on
  every update and then run an emission-timing computation whose remaining
  semantics were not established.
- An element reads the live generator frame unless its setup flags carry bit
  23, in which case it is given a private copy of the frame and fit vectors at
  birth. That private copy still follows when the element's own generator
  carries updater 0x11; it stays where it was captured only when the generator
  has no follow updater. An element of an unattached generator born with a
  parent element context takes its private frame from that context.
- An immediately linked generator (0x3C) and the child-generator links (0x44,
  0x45) inherit the parent's attachment context; 0x45 also copies the parent
  element's position.

## Element placement and billboard

- `world = frame x (local x positionFit)`: the element position (setup
  position, plus spread, plus integrated velocity) is scaled per axis by the
  position-fit vector and transformed by the frame as a point, rotation and
  translation included; the view position is the view matrix applied to that
  world point.
- The element matrix is `scale(0x0F scale as driven by the scale updaters and
  tracks) [x modelFit when the model-fit nibble is nonzero] x rotation(0x09
  Euler)`.
- A screen billboard (billboard flag set, no orientation bits) is drawn with
  its element matrix followed by a fixed half-turn about the horizontal view
  axis, translated to the view position. The frame's rotation does not reach a
  billboard; only its translation does, through the world point. A
  non-billboard element multiplies its element matrix by the frame's rotation
  with the translation cleared.

## The level-up case

- `g0s0` (sound), `g000` (lettering sheet `lvu1`) and `g001` / `g002`
  (sparkle sheet `lvu4`) are mode 1, source reference 1, fit nibbles 0;
  `g000`, `g001` and `g002` carry follow 0x11 word `0x3FD`: translation only,
  rate 255/255, so the origin snaps to the posed reference every update while
  the rotation stays the caster's yaw at activation. `g004` (the lettering
  ghost) is unattached, started from `g000` through 0x44 with the 0x45 parent
  position copy and setup bit 23, and has no follow updater, so each ghost
  stays where its parent was when it was born.
- Origin: the caster's posed reference 1 (Hume M joint 25, 1.05 above the
  feet in bind pose), in the caster's yaw frame, with no height, nameplate or
  race offset.
- `g000`: setup position (0, 0, 0); velocity (0, -0.031, 0) per 60 Hz unit
  (up) with damping 0.924 per unit, so its centre rises about 0.4 model units
  (0.031 / (1 - 0.924)), most of it within 40 units. Scale (0.08, 0.08); the
  `lvu1` quad spans 16 x 4 sprite units about its pivot, so the lettering is
  1.28 x 0.32 model units centred on the element position. Emission every 157
  updates, particle life 150 units.
- `g001`: setup position (0, -0.3, 0), 0.3 above the reference point; spread
  0x1F with radius 0.3, random extra 0.6, ellipse (1.29, 0.28, 0.18), azimuth
  range pi, flag 1; X scale 0.1 on a 2-wide quad (0.2 model units), Y scale
  growing 0.01 per unit plus a random 0.008; velocity (0, -0.003, 0); one
  element every second update, each spawning its linked copy. `g002` is that
  immediately linked copy, turned a quarter turn (the horizontal streak) at the
  parent element's position.
- Interop result for a standing Hume M at scale 1: the lettering starts
  centred at waist height (1.05, spanning 0.89 to 1.21) and rises to about
  1.46 (spanning 1.30 to 1.62) on a 1.81-tall model, so it overlaps the chest
  and shoulders and ends at the neck; it is not drawn above the head. The
  sparkle crosses are centred 0.3 above the waist, spread by the 0x1F
  parameters, each drifting up slowly. The group tracks the waist joint every
  update and inherits no height or nameplate offset.

## Other attached effects

- Hit sparks (`ROM/0/0.DAT` directory `hit1`: `g010`, `g011`, `g013` mode 2
  and `g012` mode 4, all with target reference 49): the frame sits on the
  victim's ring entry nearest the attacker, oriented by the victim's yaw (mode
  2) or the attacker's (mode 4). The contact-point placement in
  `2026-09-27-hit-effect-contact-point.md` comes from the DAT's target index
  49, not from a rule about target-side attach types; a target-side generator
  carrying a plain index in bits 10-15 attaches to that reference. Another
  global directory's `g010` / `g011` are mode 5 with source reference 50: the
  caster's ring entry nearest the target, oriented by the target's yaw.
- Casts and weapon skills use the same word: modes 1 and 2 with references
  such as 0 (feet), 1, 21 or 23/24; modes 9 and 10 with weapon references put
  the frame on the posed weapon locator (trails); modes 3 and 6 stretch a beam
  between two points.
- A large mob gets a larger effect only for generators whose fit nibbles are
  set; everything else keeps authored size at the authored offset, on an attach
  point that scales with the mob through its posed joints.

## Boundaries

- This is binary and DAT evidence, not a live retail observation. The
  `levelup.png` / `levelup.mp4` captures beside this investigation are Kuluu
  captures. The claim that retail draws the lettering above the character is
  neither supported nor refuted here; the inspected rule puts it at the posed
  waist reference rising to neck height.
- The joint heights are the bind pose; the live idle pose moves joint 25
  slightly. The sparkle band's geometry follows the existing parser's reading
  of the 0x1F spread parameters, which was not re-traced here.
- The DAT location of the two per-generator fit factors, the full semantics
  of generator updaters 0x0E-0x10, the depth/sort initializer 0x30, the
  predicates selecting the mount offset tables, the alternate actor of modes
  22-27, what an unattached child's private frame copies from its parent
  context, and the exact use of modes 7, 8, 11, 12 and 19-27 were identified
  only to the extent stated above. Which of the three riding or seated states
  is the chocobo, the mount or the chair, and which object kinds select the
  per-race tables, were not pinned; the offsets and their selection structure
  were.
- The orientation that modes 7 / 8 copy is rebuilt from the view matrix each
  frame; whether it keeps the view's roll was not pinned.
- The per-axis order in which the three per-axis scales map onto the model's
  X and Z axes was not pinned; only the vertical axis (the height scale) was,
  which is all a uniform scale or the level-up case needs.
- The element matrix order and the billboard half-turn each have a
  flag-selected variant (rotation applied before the scale; a vertical flip in
  place of the half-turn) whose DAT bit was not traced; the composition stated
  above is the path without those flags.

## Provenance

Inputs:

- `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`,
  KNOWN_CLIENTS row `retail-2026-09`, patch `30260904_1`, SHA-256
  `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`
  (rechecked). Analysis used the existing unpacked copy
  `artifacts/polre/unpacked/FFXiMain-f2245d1c.unpacked.dll` and the raw text
  `FFXiMain-f2245d1c.text.bin`, SHA-256
  `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`
  (rechecked). Image base VA `0x10000000`; `.text` RVA `0x1000`. All
  addresses below are RVAs of this build (VA minus the image base) unless
  marked VA. No installed file was modified or executed.
- Method: Capstone 5 linear sweep (skipdata) over the raw text into a listing
  searched by address and operand, function bodies read individually with
  `artifacts/polre/asm.py`; virtual calls resolved through the actor vtable
  and the math-provider vtables; a full-image scan for the GetElem pointer
  found every vtable sharing it. DAT values came from the install through
  `dat-particle-attach`, `dat-chunks` and `dat-chunk-dump` (`ffxi-dat`
  examples) and a throwaway Python walk of VTABLE/FTABLE, the chunk stream,
  the skeleton chunk (bind pose composed down the parent chain from the stored
  quaternion and translation) and the generator sections. Search hypotheses
  came from `research/XIClient/src/XIClient/source` (`World/Actor/Attachment.cpp`
  MakeEIDPoint and MakeAttachMatrix, `World/Model/ModelInstance.cpp`
  GetLocatorWorldPosition, `World/Model/ModelPartInstance.cpp`
  UpdateBoneTransforms, `World/Actor/SkeletalMeshActor.cpp` UpdateVisibility
  and the pose world matrix, `World/Generator/CYyGenerator.cpp`) and
  `research/xim` ParticleGeneratorAttachment.kt; nothing was transcribed.

Attachment functions:

- `MakeEIDPoint` RVA `0x3BA20`, args (out, index, source actor, reference
  actor, local offset). Switch on index-48 with the 13-entry table at RVA
  `0x3BD50`; default branch `0x3BD11` calls actor vtable slot `0x1C4`
  (GetElem) and adds the rotated local offset. Ground probe `0x3BB0E..0x3BB89`
  (slot `0x1C4` with index 0, 1.0 const RVA `0x32961C` subtracted, 6.0 const
  RVA `0x32A3E8` added, collision call `0x181590`). Nearest-ring loop
  `0x3BC0F..0x3BCB6`: immediate `0x4B7FFFFF` (16777215.0), ring indices 13..20,
  distance via provider slot `0x94`; reference position via actor slot `0x1BC`
  (cases 49/50) or the camera getter `0x15250` (case 51). Remaps 54-60 at
  `0x3BCCA..0x3BCF4` (`0x7E`, `0x7F`, `0x66..0x6A`). The local offset is
  rotated by the source yaw (slot `0x1C0` GetDir, provider slots `0x64` and
  `0x50`) at `0x3BA57..0x3BA7A`. The single-point modes push a null offset;
  the two-point modes pass MakeAttachMatrix's second argument to the
  source-reference call and its third to the target-reference call (mode 3:
  `0x3BE75`/`0x3C0F1`; mode 6: `0x3C140`/`0x3C15B`; 16: `0x3BF95`/`0x3BFBD`;
  17: `0x3C009`/`0x3C031`; 13: `0x3BEB5`/`0x3BEDD`; 18: `0x3BF23`/`0x3BF4B`).
  The ground case ignores the offset; the ring cases add it to every candidate.
  Fields read by cases 52/53 (`0x63C`, `0x64C` of the actor) are written in the
  actor routine at `0xD4C60..0xD4FD0`: `0x64C` is the position (slot `0x1BC`)
  with Y replaced from slot `0x220` when slot `0x218` returns 9 (`0xD4C9E`,
  `0xD4CD9`); `0x63C` is a locator point from slot `0x1C8` (indices 8, `0xB`,
  `0xA`) copied when its Y is not above the plane tested there
  (`0xD4ED3`, `0xD4F1C`, `0xD4F72`, `0xD4FBB`).
- `MakeAttachMatrix` RVA `0x3BE10`: mode = `(word & 0xF) + ((byte 6 & 1) << 4)`
  at `0x3BE84..0x3BE95`, `cmp eax, 0x1b`, 28-entry table at RVA `0x3C650`.
  Mode 1 handler `0x3C177`: index `((word >> 4) & 0x3F) + (((word >> 18) & 1) << 6)`
  at `0x3C183..0x3C194`, MakeEIDPoint call `0x3C19C` (caster as source, target
  as reference), caster GetDir slot `0x1C0` at `0x3C219`, yaw rotation
  provider slot `0x64` at `0x3C22E`, translation stores `0x3C23D..0x3C243`.
  Mode 2 `0x3C1B0` (`shr ecx, 0xa; and ecx, 0x3f`, target yaw at `0x3C282`),
  mode 4 `0x3C24D`, mode 5 `0x3C1D8`, mode 3 `0x3C0BC`, mode 6 `0x3C130`,
  modes 13/26 `0x3BEA9`/`0x3BEA7` (points ordered as mode 3, distance
  discarded), 18/27 `0x3BF17`/`0x3BF15` (ordered as mode 6, discarded; the
  helper's last argument, 1 for 13 and 0 for 18, is never read), 22-25
  `0x3C0BA`/`0x3C24B`/`0x3C1D6`/`0x3C12E` (the `mov ebx, edi` preludes of 3,
  4, 5, 6), 16 `0x3BF83`, 17 `0x3BFF7`, 7 `0x3C2B4` and 8
  `0x3C31D` (16 dwords from global object `0x1047BFA8` VA plus `0xC10`; the
  routine at RVA `0x6A180` rebuilds that matrix from the view matrix at
  `0xB50`: `0xB90` is the view with its translation cleared, `0xD10` and
  `0xD50` are reduced copies, and `0xC10` is their product via provider slot
  `0x24`), 9
  `0x3C379`, 10 `0x3C3C3`, 11 `0x3C3EF` and 12 `0x3C497` (`fpatan` on the
  locator matrix, negated, provider slot `0x64`), 14 `0x3C590` / 15 `0x3C555`
  (zone getters `0x186E90` / `0x186EB0`, static matrix `0x1773F0`, provider
  slot `0x50`, translation set `0x6AD30`), 19 `0x3C06B` (reference 2 of the
  target, 0.18 const RVA `0x32AB24` added to Y), 20 `0x3C513`, 21 `0x3C52A`,
  default identity provider slot `0xC4` at `0x3C5F0`. Two-point basis helper
  RVA `0x3B980` (difference, `fpatan` yaw and pitch, provider slot `0x58`,
  translation add, returns the distance). Locator-matrix helper for modes 9-12
  RVA `0x3BD90` (remap table `0x3BDE8`, actor slot `0x1D0`). The float return
  slot is zeroed at `0x3BE3F` and written only by the distance-returning
  modes. Caster/target getters `0x3B6D0` / `0x3B700`; the alternate actor of
  modes 26/27 comes from `0x845E0` on the caster.
- Actor vtable RVA `0x32D710`: slot `0x98` -> `0xD08E0` (width scale), `0x9C`
  -> `0xD09B0` (height scale), `0xA0` -> `0xD0A80` (depth scale), `0x110` ->
  `0x849A0` (override scale), `0x1BC` -> `0xA4740` (position), `0x1C0` ->
  `0xA4750` (direction), `0x1C4` -> `0xD4560` (GetElem), `0x1C8` -> `0xD4800`,
  `0x1D0` -> `0xD49D0` (locator matrix). The GetElem pointer sits at slot
  `0x1C4` of seven vtables: RVA `0x32D710`, `0x32E890`, `0x32ECB0`, `0x32F0D0`,
  `0x32F4F0`, `0x330F40`, `0x3313E8`, so every actor class shares the rule.
- `GetElem` RVA `0xD4560`: copies the model position (object `0x678`) to the
  output via `0x26E90`, calls the model accessor thunk `0x2C3E0` -> `0x2C400`
  with (base 0, index, out), joint word via `0x2C5C0` -> `0x2A9B0` (returns the
  entry's joint for index < `0x80`). Mount branches `0xD45E8..0xD476A`, three
  predicates each gated on the joint word being 0: (a) slot `0x358` zero and
  `0x84370(0x84390())` true -> ridden object at actor `0x768`; none ->
  `0xBFA66666` (-1.3) at `0xD473B`; else seat class via `0x22340` on object
  `0x878` with `0x84420()` -> 0 or 6 -> -1.3; else default `0xBF000000` (-0.5)
  set at `0xD464D`, replaced by a per-race table indexed by `0x84420()` when
  the object's kind `0x84720()` is `0x1D` (table at frame `0x44`: 0, 0.5, 0.5,
  0.5, 1.1, -0.7, -0.7, 0.5, 0.05) or `0x25` (frame `0x20`: 0, 0.4 x4, -0.8,
  -0.8, 0.4, 0.4); (b) slot `0x34C` zero and `0x84350(...)` true -> -1.3 at
  `0xD473B`; (c) slot `0x36C` zero and `0x84330(...)` true -> -0.5 at
  `0xD476A`. The offset is zero-initialised at `0xD45D9`; scale
  choice slot `0x110` / object `0x75C` / `0x754` at `0xD4772..0xD47D9`; `fmul`
  then `fadd` into out Y at `0xD47D9..0xD47E7`.
- Model accessor RVA `0x2C400`: `cmp edi, 2` at `0x2C40C`; special branch
  `0x2C417..0x2C4AF` walks the part list (`0x2B5A0`), calls `0x2A8B0` (adds the
  entry translation, record offset `0xE`) with index 2, multiplies by the
  model scale fields at object `0x24`, `0x2C`, `0x28` (entry x, y, z) via
  `0x27310`, adds to the output via `0x26F20`. General branch `0x2C4B2`:
  `test byte ptr [esi + 0x30], 0x10` then `0x2A750` (zeroes the output through
  `0x26E10`) -> `0x2A780`: record lookup `0x351F0`, identity `0x279B0`, rotate
  X/Y/Z from record offsets `2`, `6`, `0xA` (`0x27B80`, `0x27BD0`, `0x27C20`),
  translation row from record offset `0xE` (`0x27CF0`), multiply by the bone
  matrix at skeleton object `0x14` plus joint index times 64 (`0x27D10`),
  translation extracted by `0x28140` -> `0x28170` (row-vector point transform
  of the zero vector).
- Reference-table lookup RVA `0x351F0` / `0x35210`: joints are 30 bytes from
  resource offset `0x34` (count at `0x32`), the reference table follows a
  4-byte count with 26-byte entries; `0x35250` returns the first bounding box
  (24-byte stride, used by the fit getters).
- Pose evaluation: the actor routine spanning RVA `0xCC5C0..0xCC966` selects
  the per-axis scale at `0xCC6B8..0xCC7EB` (slot `0x110`, fields `0x760`,
  `0x75C`, `0x758`, `0x754`), builds the scale matrix with `0x27AE0` into
  object `0x998` at `0xCC81A`, rotates with `0x27B80` / `0x27BD0` / `0x27C20`
  from slot `0x1C0` plus object `0x62C`, sets the translation with `0x27CD0`
  from slot `0x1BC` plus object `0x60C`, and calls the model pose routine
  `0x2C2D0` at `0xCC950` with that matrix (model object `0x674`); a second
  selection at `0xCCA13..0xCCBBD` writes the same matrix at `0xCCBEC`.
  `0x2C2D0` walks the part list calling `0x2A140` per part with the matrix and
  then sets `or al, 0x12` on model object `0x30` at `0x2C311`, which includes
  the `0x10` bit the locator accessor tests.
- Fit-scale getters: `0xD08E0` (part list `0x2B5A0`, box `0x35250`, extent
  `[eax + 8] - [eax + 0xc]`, scale selection slot `0x110` / `0x760` / `0x754`,
  const RVA `0x331394` = `0x3F169696` = 1/1.7); `0xD09B0` (extent
  `[eax + 4] - [eax]`, const RVA `0x331398` = `0x3F06BCA2` = 1/1.9, selection
  `0x75C`); `0xD0A80` (extent `[eax + 0x10] - [eax + 0x14]`, same constant,
  `0x758`). The selection idiom is `fcomp` against zero with `test ah, 5; jp`,
  taking the getter or field when it is not negative.

Math provider (global pointer at VA `0x1047CF7C`, written at RVA `0x6CBAB`,
`0x6CBE9`, `0x6CC11`; vtables RVA `0x32C368` generic, `0x32C298` and `0x32C1C8`
specialised): constructor `0x6CE70` copies an identity into the scratch
matrices at object `0x810` and `0x850`. Slot `0x20` -> `0x6CC40` forwards to
slot `0x24` -> `0x6D270`: `out = A x B` in the row-vector convention
(`out[i][j] = sum A[i][k] * B[k][j]`). Slot `0x50` -> `0x6D720`: point
transform including the translation row. Slot `0x64` -> `0x6E140`: rotation
about Y by the given angle. Slot `0x74` -> `0x6E710`: writes `cos`/`sin` into
the second and third rows of the scratch matrix at `0x850` (a rotation about
X) and forwards to slot `0x20` with (out, element matrix, scratch). Slots
`0x58` -> `0x6DCF0`, `0x6C` -> `0x6E380` (Euler), `0x94` -> `0x6D580` (length),
`0xC4` -> `0x6D0A0` (identity). The specialised vtables point the same slots
at other bodies except `0x74`, which is shared.

Generator functions:

- `AttachCalc` RVA `0x53E60`: zeroes object `0x90`, mode test `0x53E74..0x53E85`
  (mode 0 returns false), frame via vtable slot `0x28` (`0x53E00`: allocates
  the 64-byte matrix at object `0xA4`, identity, fit vectors `0x78..0x8C` =
  1.0), MakeAttachMatrix call `0x53F0E`, return stored to `0x90`. Before the
  call (`0x53E8B..0x53EFE`) the section-2 block with opcode `0x7B` is looked
  up through `0x492F0` (walks the list at generator object `0xE4`; `0x492B0`
  walks section 1 at `0xE0`), cached at object `0x98`, and its payload words
  1-3 and 4-6 are copied to two stack vectors (w = 1.0) passed as
  MakeAttachMatrix's second and third arguments; without the block both are
  null (`0x53F07`). Tracking does the same at `0x53CC6..0x53D18`. Position-fit
  nibble `shr ecx, 0x14; and ecx, 0xf` at `0x53F1D..0x53F20`, tables `0x541A0`
  / `0x541A8` (1-4 caster `0x3B6D0`, 5-8 target `0x3B700`), factor object
  `0x70` at `0x53F5E` (1.0 when zero), width slot `0x98` / height slot `0x9C`,
  writes `0x78`, `0x7C`, `0x80`. Model-fit nibble byte object `0x37` at
  `0x54042`, factor `0x74`, writes `0x84`, `0x88`, `0x8C`, tables `0x541B0` /
  `0x541B8`. Distance override `0x54171..0x5418C`.
- Activation routine containing RVA `0x5330C`: task FourCC `0x4D670000` stored
  at `0x53305`, AttachCalc call `0x5330C`, mode-based task linking
  `0x53311..0x5333A` (compares `0x14`, 1, `0xE`, `0xF`).
- `Tracking` RVA `0x53C80`, args (block, matrix to blend): rate
  `(word >> 2) & 0xFF` at `0x53C8B..0x53C99` times const RVA `0x329E94`
  (`0x3B808081` = 1/255), MakeAttachMatrix call `0x53D22`, distance override
  `0x53D27..0x53D46`, translation blend `0x53D49..0x53D73` (bit 0), rotation
  blend `0x53D75..0x53DA1` (bit 1). Wrapper `0x53DD0` finds opcode 0x11 via
  `0x492B0`. Callers: generator Idle `0x496F0` (section-1 dispatcher
  `opcode - 4` at `0x49794..0x497A7`, table RVA `0x4A814`, index 13 = opcode
  0x11 handler `0x4A533`, which sets bit 9 of generator object `0xD8` with
  `or dh, 2` and calls `0x53C80` at `0x4A550` on the generator frame `0xA4`;
  indices 10/11/12 = opcodes 0x0E/0x0F/0x10 handlers `0x4A0B6`, `0x4A236`,
  `0x4A3B6` each calling AttachCalc at `0x4A0B8`, `0x4A238`, `0x4A3B8`) and
  the element update `0x44AE0`, which tests that bit 9 at `0x44BAD` /
  `0x44BD4` and calls the wrapper at `0x44BE6` with the element's frame
  accessor (`0x473F0`) result. No instruction clearing bit 9 of `0xD8` was
  found in the text. Other AttachCalc callers: `0x5007A`, `0x59FD7`.

Element functions:

- Setup block 0x01 handler: initializer table RVA `0x5247C` entry 0 ->
  `0x50D26`. Bit-23 test `test dword ptr [ebp + 4], 0x800000` at `0x511C6`;
  frame-present predicate `0x52AF0` (generator `0xA4` non-null); private copy
  allocation `0x511E2..0x5120A` (`0x60` bytes at element `0x108`: the
  64-byte frame from `0x52B00`, then the fit vectors through `0x47450` /
  `0x47490`); parent-context path `0x512BD..0x512E2` filling the copy through
  `0x527E0`. Payload words 4-6 (block `0x14`, `0x18`, `0x1C`) are stored to
  element `0x54`, `0x58`, `0x5C` at `0x513DF..0x513F0`. The element
  constructor zeroes `0x108` at `0x44903`; the destructor frees it at
  `0x44A6D`.
- World position RVA `0x44C40`: copies element position object `0x54`, frame
  accessor `0x473F0` (element `0x108` birth copy, else the generator at
  `0x100` or `0xFC` and its `0xA4`), position-fit accessor `0x47450` (birth
  copy plus `0x40` else generator `0x78`), transform via provider slot `0x50`.
- `CalcTrans` RVA `0x44F10`: flag bit 2/3 paths `0x44F2A..0x44FC6`, bit 17 path
  `0x44FCB..0x4509B`, default `0x4509D..0x450CC` (world position then view
  matrix at global object `0xB50`), writes `0x188`; depth `0x12C` from `0x128`.
- `CalcMatrix` RVA `0x44D40`: identity into `0x60`, scale from `0xEC`, model-fit
  gate `test byte ptr [eax + 0x37], 0xf` at `0x44D88` and `0x44E74` with
  accessor `0x47490` (birth copy plus `0x50` else generator `0x84`), Euler
  `0xE0` via provider slot `0x6C` or `0x58` (flag bit 9 of `0x10C`), multiply
  slot `0x20` at `0x44E41`.
- Billboard composition RVA `0x45D40` (the billboard draw of
  `2026-10-02-level-up-linked-sparkle.md`): flag test `0x45D62..0x45D7B`;
  billboard branch `0x45DF6` (provider slot `0x74` with immediate
  `0x40490FD8`, pi, on the element matrix `0x60` into `0xA0`; translation from
  `0x188..0x190` at `0x45E11..0x45E29`); non-billboard `0x45EF5..0x45F85`
  (frame rotation with translation cleared from `0x45C50`, slot `0x20`);
  near-plane clamp const RVA `0x32A39C` (0.2). Fit-scaled position copy
  `0x45B39..0x45B7A` into `0x90`.

DAT measurements (install `retail`, VTABLE/FTABLE walk; chunk offsets are
file offsets of the 16-byte chunk header; body offsets exclude it):

- File 3310 -> `ROM/13/35.DAT`. Generator chunks: `g002` at `0x20`, `g001`
  `0x1A0`, `g004` `0x360`, `g000` `0x4B0`, `g0s0` `0x630`. Attach words
  `0x0011` / `0x0000` (`g004`: `0x0000` / `0x0000`). Section 1 of `g000`,
  `g001`, `g002`: one block, opcode 0x11, payload word `0x3FD`; `g004` and
  `g0s0` have an empty section 1. Setup block 0x01 payload words: `g000`
  flags `0x4001`, position (0, 0, 0), word 7 `0x00960E20` (life 150); `g001`
  flags `0x1`, position (0, -0.3, 0), word 7 `0x001E0E2C` (life 30); `g002`
  flags `0x1`, position (0, 0, 0); `g004` flags `0x800001`. `g000`: 0x02
  (0, -0.031, 0), 0x30 -0.3, 0x09 (0, 0, 0), 0x0F (0.08, 0.08, 1), 0x1E
  `0x44`, 0x16 `0x808080`, 0x2D -> `k000`, 0x44 -> `g004`; section 3: 0x0E,
  0x02, 0x2C 0.924 (`0x3F6C8B54`), 0x0D, 0x1B, 0x25; emission word 156.
  `g001`: 0x02 (0, -0.003, 0), 0x1F (0.3, 0.6, 1.29, 0.28, 0.18, 0, 0, 0,
  3.1415, 1, 0), 0x30 -0.5, 0x0F (0.1, `0x34380000`, 1), 0x12 (0, 0.01, 0),
  0x13 (0, 0.008, 0), 0x27 -> `k001`, 0x3C -> `g002`; emission word 1.
  `g002`: 0x45, 0x02 (0, -0.003, 0), 0x09 (0, 0, 1.57075), 0x0F (0.1,
  `0x34380000`, 1), 0x12, 0x13 as `g001`. `g004`: 0x45, 0x30 -0.2, 0x0F
  (0.08, 0.08, 1), 0x2D -> `k002`. `g0s0`: 0x4C (60, 0, 0); emission word 100.
  Sprite sheets (type 0x21, one quad of six {position, colour, uv} vertices):
  `lvu1` x -8..7.9375 (`0xC1000000`..`0x40FE0000`), y -2..1.9375
  (`0xC0000000`..`0x3FF80000`); `lvu4` x -1..0.9375 (`0xBF800000`..`0x3F700000`),
  y -2..1.9375.
- Skeleton chunks (type 0x29; joints 30 bytes: parent u8, pad, quaternion
  4 f32, translation 3 f32; references 26 bytes: joint u16, rotation 3 f32,
  translation 3 f32; then 24-byte bounding boxes). File 7072 -> `ROM/27/82.DAT`
  `hum_`, 94 joints, 128 references, 5 boxes. Entry 1: joint 25, zeros;
  bind-pose point (0.047, -1.053, 0). Entry 2: joint 0, (0, -2.0, 0). Entry 3:
  joint 51, point (-0.003, -1.503, 0). Entry 12: joint 0, (0, -1.81, 0). Entry
  21: joint 0, (0, -1.24, -0.04). Entries 13-20: joint 0 at y -1.1, radius
  0.17-0.32. Entries 49-51: joint 0, zeros. Entries 126/127: joints 85/68,
  points (0.049, -0.838, +-0.308). First box: y -1.9..0, x -0.7..0.7,
  z -0.7..0.7. Other skeletons (file -> path, name, entry 1 joint / bind
  height, entry 2 height, entry 12 height, first-box y / x half-extent):
  10248 -> `ROM/32/58.DAT` `huf_` 3 / 0.993, 2.0, 1.68, 2.0 / 0.8; 13424 ->
  `ROM/37/31.DAT` `elv_` 3 / 1.222, 2.4, 2.09, 2.26 / 0.8; 16600 ->
  `ROM/42/4.DAT` `elv_` 3 / 1.125, 2.4, 1.97, 2.26 / 0.8; 19776 ->
  `ROM/46/93.DAT` `tar ` 72 / 0.300, 1.3, 1.00, 1.04 / 0.4; 23176 ->
  `ROM/51/89.DAT` `mit ` 3 / 0.993, 2.0, 1.68, 2.0 / 0.8; 26352 ->
  `ROM/56/59.DAT` `gal ` 3 / 1.170, 2.6, 2.30, 2.5 / 1.0. Heights are the
  negated bind-pose Y from quaternion/translation composition down the parent
  chain; the fit values in the Scale section are (2 x half-extent) / 1.7 and
  (y extent) / 1.9.
- File 0 -> `ROM/0/0.DAT`: `g013` at `0x5B460` word `0xC402`, `g012` `0x5B5F0`
  `0xC404`, `g011` `0x5B790` `0xC402`, `g010` `0x5B8F0` `0xC402`; `g010`
  `0x71C50` and `g011` `0x71FA0` `0xC725`.
- Opcode 0x7B sample (section-2 walk over `ROM/0`, `ROM/13`..`ROM/16`: 630
  files, 26398 generator chunks, 211 in a two-point mode): 20 generators carry
  the block, all two-point, each 7 words. `ROM/14/112.DAT` `gp00` at `0x710`
  (mode 3): (0, 0, 0), (0, -2.2, 0); `ROM/14/116.DAT` `pk00` at `0x3F0`
  (mode 3): (0, 0, 0), (0, -2.83, 0). File 3310 has none.
