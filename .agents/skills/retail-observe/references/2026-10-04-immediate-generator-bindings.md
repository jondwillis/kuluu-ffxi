# Immediate generator bindings, 2026-10-04

## Supported rule

An immediate linked generator uses its own initializer and updater scripts.
The newborn parent element is context, rather than a substitute for the linked
generator's authored resource bindings. This distinction applies to position
tracks, the damping-factor binding and further child-generator references.

This record identifies script ownership on `retail-2026-09`. It does not
establish complete updater arithmetic, coordinate conversion, emission timing,
attachment transforms or presentation parity. An explicit script-redirection
opcode is a separate operation and is outside this conclusion.

## Binary evidence

Input is the registered retail install's original `FFXiMain.dll`, SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
The independently decoded text has SHA-256
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Image base is VA `0x10000000`; the raw text begins at RVA `0x1000`.
Hashes were rechecked against the installed DLL and the previously independently
decoded text. No installed file was changed or executed.

Initializer dispatch uses the opcode-minus-one table at RVA `0x5247C`.
Opcode `0x3C` selects RVA `0x4EE0E`: it dereferences the linked generator at
instruction +4, clears its countdown, and calls Idle RVA `0x496F0` with that
generator in ECX and the newborn parent element as argument. Idle's emission
call at RVA `0x4A61C..0x4A623` passes its generator to element creation RVA
`0x4E8D0`, which reads the initializer script at generator +`0xE4`.
Position-track initializers `0x21/0x22/0x23` select RVAs
`0x504C1/0x50707/0x50740`; damping-factor initializer `0x69` selects RVA
`0x50B27`; child-generator initializer `0x44` selects RVA `0x4EE4A`.
They are instructions in the selected generator's initializer script.

The element's update at RVA `0x44AE0` loads its generator reference through
element +`0xFC`; the call at RVA `0x44BF2` reaches ElemIdle RVA `0x4A980` with
that generator in ECX. ElemIdle reads updater script +`0xE8` at RVA `0x4A9BD`
and element-local bound data through element +`0x176`. Its opcode-minus-one
table at RVA `0x4E03C` maps position updaters `0x0F/0x10/0x11` to RVAs
`0x4B975/0x4B9E8/0x4BA40`, and damping-factor updater `0x44` to RVA
`0x4BFDF`. Those handlers use the updater instruction's bound-data index.
The Y/Z handlers write position +`0x58/+0x5C` after evaluating the bound track.
The damping handler writes the sampled value into the initialized damping
binding. There is no implicit substitution of the parent generator's script
in the inspected immediate-dispatch path.

The community map used for identification was
`research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp`
(`ElemGenerate`, `ElemIdle`), together with
`research/XIClient/src/XIClient/source/World/Generator/Effects/CMoElem.cpp`
(`Idle`). Capstone decoded the original-build instructions independently.
The local raw evidence is
`/private/tmp/kuluu-immediate-bindings-retail-contract.log`.

## Kuluu reproduction boundary

The regression drives `spawn_particle_generators` with distinct, synthetic
parent and linked definitions through `ActionAssets`, then exercises production
emission, draw-position evaluation and damping. At half lifetime, the old
linked path produces the parent's position `(-3, -4, -5)` instead of the
linked definition's `(1, 2, 3)`. It also retains parent child factories.
These are controlled valid-resource inputs, not an assertion that the same
parameter combination occurs in every shipped effect. Runtime captures of
this synthetic scene can verify the correction's rendered consequence;
they cannot verify a live session or the complete retail effect.
