# Indexed texture row order

Build: retail-2026-09, FFXiMain.dll SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Unpacked .text SHA-256: `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.

The indexed texture upload at RVA 0x39FF2 reads height at object +0x26,
subtracts the output row and one, multiplies by width, adds PixelData (+0x20)
and column, then looks up the index in PaletteData (+0x1C). Output rows advance
forward. Indexed payloads are bottom-up; decoder output must reverse rows while
preserving columns. XIClient GameTexture.cpp corroborates the binary finding.

DAT file 3310 resolves through the retail registry to ROM/13/35.DAT. Its lvu1
Img is B1, 256x64. The lvu1 sheet at file offset 0x6680 has top vertices y=-2,
v=0 and bottom vertices y=1.9375, v=0.984375. Raw decoded rows show inverted
lettering. Correcting row order produces upright Level Up!! without changing
authored meshes, UVs or particle transforms.

Local evidence: /private/tmp/pr804-levelup-fix. The image-target-only Bevy probe
uses production scheduling, simulation and materials, with no window, audio or
server. Before/after frame-60.png show the correction. lvu2/lvu3/lvu4 DXT
textures remain byte-identical; sparkle pixels match at frames 60, 90 and 120.
This verifies texture orientation, not every effect opcode or full live parity.
