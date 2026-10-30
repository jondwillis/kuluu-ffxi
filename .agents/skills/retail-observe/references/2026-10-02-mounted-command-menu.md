# Mounted command menu, retail-2026-09

Static binary inspection of the registered retail install (see Provenance); not a
live-client capture.

## Dismount confirmation

The command-menu dispatch obtains the string pair for "Dismount chocobo?" and
opens the yes/no query with the cursor starting on **No**. Only an affirmative
answer sends anything, and what it sends is the literal `/dismount` command.
Therefore confirmation exists and starts on No, not Yes.

The menu's dig entry issues `/dig` directly - no confirm at all. Permission is
the server's boundary.

## Mounted menu scope

Mounted contexts are separate action lists rather than ordinary menus with a
Dismount row appended. The self context includes Dig and Dismount; non-self
contexts differ. The owner-provided rented-chocobo screenshot in
`2026-09-24-chocobo-mounted-menu.md` independently establishes the visible self
menu's Chat / Dig / Dismount rows - chocobos included.

Dig's client predicate rejects certain actor animation states and applies zone
and actor flag checks through further routines; this trace does not establish
all terrain and mount-ID rules. Do not equate a chocobo-shaped model with
digging permission. For the server permission boundary, LandSandBoat's
`vendor/server/src/map/packets/c2s/0x01a_action.cpp`
`GP_CLI_COMMAND_ACTION::process` accepts ChocoboDig only for MOUNT_CHOCOBO.

Live retail interaction was unavailable: the observation doctor reported a
locked screen and no client window. Exact on-screen confirmation placement,
non-self interaction, and the remaining Dig predicates are not observed here.

## Addendum 2026-10-29: the get-off routines in DAT (static)

`dat-routine-stages` over the install's own files settles both sides of what
plays when a rider leaves:

- Rider-side, action-animation block base (+0; HumeM file 38603): routine `coff`
  = HoldRotation+MovementLock+AnimationLock (all 180 frames), Motion `ch1?` (win
  104) then `ch2?` (72). The hop-down and land.
- Mount-side, yellow chocobo race config (file 55295): routine `coff` = Motion
  `ch1?` (104), then at frame 100 Motion `run?` dur 32 with authored max_loops=2,
  a hold slot to ~174, and an ActorFadeOnCaster stage at frame 150 (dur 12) - the
  gallop-away and its fade-out despawn cue. The other chocobo colour configs
  carry the same family.
- research/xim `poc/game/event/ActorDismountEvent.kt` plays exactly that
  mount-side `coff` when a rider leaves (rider itself only transitions to idle
  there).

Kuluu delta after this pass: rider hop-down→land staged on the mounted→none wire
edge with keys held for its window (`DISMOUNT_ROUTINE`,
`.agents/skills/retail-observe/references/` census `ffxi-dat/examples/zz-mount-clips.rs`);
mount kept alive one authored gallop before despawn (`MountDeparting`,
`start_mount_off_gallop`); the mounted self menu is Chat / Dig / Dismount for
**any** mount - chocobos included, per the 2026-09-24 rented-chocobo screenshot
above (a kuluu gate had narrowed it to non-chocobo mounts and closed the menu on
chocobos; fixed, pinned by `a_riding_chocobo_keeps_its_self_menu_open`). Not
matched: retail's 12-frame fade-out despawn (no per-actor fade exists in kuluu)
and root motion in the gallop-away loop (kuluu plays it standing at the dismount
spot).

## Provenance

Retail install inspected 2026-09, FFXiMain.dll SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`; freshly
unpacked `.text` SHA-256 `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`;
ImageBase `0x10000000`, all VAs below.

- Command-menu dispatch VA `0x1012BAE4`: obtains string group 14 entry 77
  ("Dismount chocobo?"), calls query setup `0x10210E30` with callback
  `0x1012BCA0`; the setup selects one-based row 2 through `0x10118E90` at
  `0x10210ECB`; answer routine `0x10211398` sends true only for row 1. The
  callback issues literal `/dismount` only on true.
- Dig dispatch literal `/dig` at VA `0x1012BACF`.
- Mounted-context selector VA `0x1012B5AD` (state read via `0x10095680` and
  `0x100956A0`). Menu builder VA `0x1012B2D0` reads separate 13-byte action
  lists at `0x1036F390`, each filtered through `0x1012AF30`.
- Dig predicate starts VA `0x1012B0E5`; rejects actor animation state `0x55` via
  `0x100956A0`, then zone/actor flag checks and consultation of `0x101CC4C0` /
  `0x10177750`.

2026-10-29 census: `cargo run -p ffxi-dat --example dat-routine-stages -- <file> ""`
over DAT file ids 38603 (rider coff) and 55295/55329 (chocobo coff family); ActorFadeOnCaster is scheduler opcode `0x29`, the pre-fade hold slot is opcode `0x6E`.
