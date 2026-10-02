# Level-up linked sparkle, 2026-10-02

Input is clean `retail`, client `retail-2026-09`, patch `30260904_1`.
FFXiMain.dll SHA-256 is
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`;
unpacked text SHA-256 is
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Image base is VA `0x10000000`, text starts at RVA `0x1000`.

## Authored layers

DAT3310 resolves to `ROM/13/35.DAT`. Generator `g001` at chunk offset
`0x1A0` links `g002` through initializer `0x3C`. Both link the `lvu4`
sprite sheet: one flat XY frame with X bounds `-1..0.9375`, Y bounds
`-2..1.9375`, and Z zero. Both author X scale about `0.1`, initially
near-zero Y scale, then Y growth and variance through `0x12/0x13`.
`g001` starts at rotation zero; `g002` at chunk offset `0x20` authors
Z rotation about pi/2 through `0x09`, and parent position copy `0x45`.
The vertical and horizontal layers are distinct authored particles;
adding an arbitrary shimmer or changing the source quad is unnecessary.

## Native immediate emission

Generator element initialization at RVA `0x4E8D0` dispatches through the
opcode-minus-one table at RVA `0x5247C`. Entry `0x3C` points to RVA
`0x4EE0E`: it dereferences the resolved generator at initializer +4,
clears that generator's countdown +`0xCC`, and calls its Idle at RVA
`0x496F0` with the newborn parent element. Idle forwards this element to
emission at RVA `0x4E8D0` (RVA `0x4A61C` or `0x4A69F` onward).
This is immediate linked emission, not an independently scheduled child.
Entry `0x44` instead points to RVA `0x4EE4A`, clones through RVA
`0x536C0`, and retains its own child-generator relationship.

Initializer `0x45` points to RVA `0x4F8E5`, copies attachment context
and its matrix where present, then adds parent position +`0x54/58/5C`
to the child's own initialized position at RVA `0x4FC49..0x4FC68`.
It does not copy the parent's authored scale or rotation. The post-init
copy at RVA `0x523D8` retains camera-space sort/depth metadata; it is
not the child's local position initializer.

## Authored billboard rotation

Initializer `0x09` at RVA `0x5163F` initializes the element Euler vector
at +`0xE0`. Matrix construction at RVA `0x44DE9..0x44E41` reads its
X/Y/Z components and multiplies the resulting rotation into the element
matrix +`0x60`, without testing linked emission. Screen billboard drawing
at RVA `0x45D40` tests billboard flags +`0x10C` (bit 0 with orientation
mask `0x1C0` zero). At RVA `0x45DF6..0x45E0E`, it passes that element
matrix to the graphics matrix conversion with pi; it does not replace it
with a camera-only identity. Thus retaining authored rotation belongs to
ordinary screen billboards too, not only immediate-linked particles.

## Boundaries

Kuluu previously discarded initializer `0x3C`; its separate `0x44/0x53`
fields were parsed but not consumed. The focused correction resolves
scheduled immediate links, emits the linked particle at the parent birth,
and retains its own rotation/scale. Missing resources and cyclic resource
links stop resolution without unbounded allocations. The cloned persistent
child-generator opcodes remain outside this correction.

Canonical retail observation doctor found the macOS desktop locked and
no client window; a configured `lsb-wine` profile exists, but input cannot
be driven while locked. Binary/DAT evidence establishes this missing layer;
it does not establish full live retail presentation or mixer parity.
