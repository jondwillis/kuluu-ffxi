# Level-up scheduled emission and coordinate frame

Install profile: retail-2026-09, patch30260904_1, FFXiMain.dll SHA-256
f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4.

DAT3310 resolves to ROM/13/35.DAT. Its main routine schedules the g000
lettering generator at frame1 with duration0. The generator links lvu1,
authors emission period157, particle lifetime150 and velocity Y=-0.030999966.
The lvu1 sheet has top vertices Y=-2 and bottom vertices Y=1.9375.
These are DAT observations, not observations of retail runtime timing.

On focused PR810 head4779e004, a production scheduler/simulation/material
image-target demo emitted thin sparkle slivers but no lettering. A generator
whose first emission waits157frames cannot emit inside its zero-frame window.
Treating a zero-duration scheduled generator as one immediate burst makes its
lettering visible. This first-burst policy is an inference from the authored
schedule; precise retail first-frame timing still needs binary/live corroboration.
The policy applies only to scheduled zero-duration generators, leaving periodic
and auto-run zone generators unchanged. A long first frame must emit once,
not skip the effect or catch up through repeated bursts.

With emission restored, the scheduled particle's unconverted DAT coordinates
make the lettering descend and draw inverted. Converting its world-space
velocity/template basis through scene::mzb_to_bevy makes it rise and read upright.
This reuses the established DAT-to-Bevy transform; it does not modify meshes,
UVs, indexed texture decoding or camera policy.

The isolated example verifies visible lettering and motion in Kuluu. It does
not verify server LevelUp dispatch, retail timing, attachment to a posed player,
child generators or the remaining thin sparkle geometry. No raw game content
is stored with the example.
