# Melee hit effect placement on the victim — 2026-09-27

Question: where does retail place a melee hit's flash/dust on the struck actor?

## Observation (user, live retail client)

- The effect is NOT at the victim's feet. It is offset from the victim: lower on
  shorter mobs (height scales with mob size), roughly at their center/torso height,
  biased toward the attacker — "in the direction of the player". Standing behind a
  mob and hitting it puts the effect on the back side of the mob.
- Earlier retail reference screenshot in this thread: flash starburst at the contact
  point (lizard's mouth), dust spreading from there.

## Implication for kuluu

Placement is the victim's ring locator nearest the source actor (the contact point):
ring references 13..20 are part of the skeleton, so they scale with model size and sit
at torso height; nearest-ring selection picks the side facing the attacker. This is the
rule `attach_joint_reference` applies to target-side attach types carrying a plain EID
index (kuluu-render/src/particle_sim.rs).

## Why C++ cannot settle it

research/XIClient Attachment.cpp MakeEIDPoint implements only mode 1 in the decompile;
the hit sparks' generators use modes 2/4/5, whose matrix construction is not
decompiled. The user observation above is the spec for those modes.
