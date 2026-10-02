# Action confirm, movement locks, and the ranged sequence, 2026-09-21

Retail client behaviour observed on HorizonXI (KNOWN_CLIENTS row horizonxi-2023)
over long-term play; recorded here so kuluu code can cite it by heading.

## Menu actions always confirm through the sub-target cursor

Selecting a spell, ability, weapon skill, ranged attack or usable item from a
menu always shows the flashing sub-target cursor before anything fires. This
includes SELF-only actions: Mighty Strikes or Berserk from the Abilities menu
put the cursor on the player and wait for Enter. A valid current target seeds
the cursor (it starts there) but never skips it. Esc returns to the menu with
its cursor preserved.

## Typed commands: a target token bypasses the cursor, no argument prompts it

`/ja "Mighty Strikes" <me>` and `/ma "Cure II" <t>` fire without the cursor.
The same command with no target argument, or with `<st>`, opens the cursor.
`<stpc>`, `<stnpc>`, `<stpt>`, `<stal>` open it with the candidate set
narrowed to players, NPCs, party, alliance. `<bt>` is the current battle
target, `<pet>` the player's own pet.

## Players are never movement-locked by a cast or a ranged aim

A player can walk during a cast or an aim; doing so is what interrupts the
action ("You move and interrupt your casting/aim"), server side. Mobs stand
still while casting or aiming because the server issues no movement in their
action states, not because the client locks them.

## The one real player lock is the weapon draw and sheathe

Going from idle to battle stance (weapon draw) and back (sheathe) holds the
player in place for the length of the transition. Movement input is ignored
until the weapon is fully out or fully away.

## Ranged attack sequence

The draw/aim pose starts when the server's ranged-start action arrives, not on
the key press. Moving during the aim cancels it server side and the aim pose
drops. When the aim completes the shot motion plays and the projectile flies.
