# Level-up scheduled emission and coordinate frame

Install profile: retail-2026-09, patch30260904_1, FFXiMain.dll SHA-256
f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4.

DAT3310 resolves to ROM/13/35.DAT. Its main routine schedules the g000
lettering generator at frame1 with duration0. The generator links lvu1,
authors emission period157, particle lifetime150 and velocity Y=-0.030999966.
The lvu1 sheet has top vertices Y=-2 and bottom vertices Y=1.9375.
These are observations of the clean retail DAT.

On focused PR810 head4779e004, a production scheduler/simulation/material
image-target demo emitted thin sparkle slivers but no lettering. A generator
whose first emission waits157frames cannot emit inside its zero-frame window.
The retail-2026-09 binary establishes the missing first-burst rule. Addresses
below are RVAs relative to image base 0x10000000. Opcode 0x02 dispatch enters
0x598E1. The clone call at 0x59B10 reaches 0x536C0, which invokes activation 0x531D0
at 0x537CC. The non-continuous, non-flag-12 path then calls
0x5E590 to read signed scheduler duration at stage+6 multiplied by task+0x9C.
Setter 0x5E160 writes generator+0xD0. The handler reads it back and compares
with zero at 0x59B6B; a zero value is replaced with float 1.0 at 0x59B7C.
Predicate 0x539E0 tests flags bit 0x400 or initializer lifetime zero; predicate
0x5E190 tests flags bit 12. Clean DAT g000 has flags 0 and initializer lifetime
150, so it takes this duration-clamping path.

Activation 0x531D0 clears EBX and resets emission countdown+0xCC and age+0xC8
at 0x5324D/0x53253. Idle 0x496F0 subtracts a positive engine delta from that
zero countdown at 0x4A67C before aging the generator. The negative countdown
enters element emission at 0x4A699; it then adds signed interval base+1 plus
random variance at 0x4A6DA-0x4A707. DAT g000 authors base 156 and variance 0,
so its interval is 157. Ordinary delta is capped to 10 at 0x4A63D (a separate
flagged scaling path exists). Age advances afterward at 0x4A7A8, then reaching
the clamped duration expires the generator at 0x4A7D2. Thus a zero-duration
g000 emits on its first positive update, before expiry, rather than waiting
157 units. These are engine timing units, not a claim that one update equals
one display frame on every host.

Kuluu's zero-duration scheduled branch emits one authored batch on the first
tick, including a long first tick, without charging newly born particles the
whole tick. For g000, one bounded batch agrees with the native countdown and
157-unit interval. Kuluu does not reproduce the full native countdown/clamp
algorithm for other generator populations; this correction leaves periodic
and auto-run generators unchanged. A Kuluu hitch test proves its bounded
behavior, not exact native scheduling or presentation latency during a hitch.

With emission restored, the scheduled particle's unconverted DAT coordinates
make the lettering descend and draw inverted. Converting its world-space
velocity/template basis through scene::mzb_to_bevy makes it rise and read upright.
This reuses the established DAT-to-Bevy transform; it does not modify meshes,
UVs, indexed texture decoding or camera policy.

The production real-DAT fixture now spawns a posed HumeM render-actor child
and checks g000's origin against its authored source joint, then verifies one
bounded burst across a long first tick, rising Bevy motion, authored damping
and expiry without re-emission. The isolated example verifies visible upright
lettering and motion in Kuluu. Neither proves live server delivery, retail
presentation latency, full posed-player appearance, child generators or the
remaining thin sparkle geometry. Those broader comparisons are separate from
the corrected zero-window emission and world basis. No raw game content is
stored with the example.
