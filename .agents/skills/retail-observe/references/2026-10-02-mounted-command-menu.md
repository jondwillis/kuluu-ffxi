# Mounted command menu, retail-2026-09

Inspected the registered retail install's FFXiMain.dll (SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`).
Its freshly unpacked `.text` matches SHA-256
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
ImageBase is `0x10000000`; all addresses below are VAs. This is static binary
inspection, not a live-client capture.

## Dismount confirmation

The command-menu dispatch at `0x1012BAE4` obtains string group 14, entry 77
(`Dismount chocobo?`) and calls the query setup `0x10210E30` with callback
`0x1012BCA0`. The setup selects one-based row 2 through `0x10118E90` at
`0x10210ECB`. The answer routine at `0x10211398` sends true only for row 1;
all other rows send false. The callback issues literal `/dismount` only for
true. Therefore confirmation exists and starts on **No**, not Yes.

The menu command dispatch issues `/dig` directly at `0x1012BACF`.

## Mounted menu scope

The context selector at `0x1012B5AD` distinguishes mounted state using
`0x10095680` and `0x100956A0`. The menu builder at `0x1012B2D0` reads separate
13-byte action lists at `0x1036F390`, filtering each through `0x1012AF30`.
Mounted contexts are separate lists rather than ordinary menus with a
Dismount entry appended. The self context includes Dig and Dismount;
non-self contexts differ. The owner-provided rented-chocobo screenshot in
`2026-09-24-chocobo-mounted-menu.md` independently establishes the visible
self menu's Chat / Dig / Dismount rows.

Dig's predicate starts at `0x1012B0E5`; it rejects actor animation state
`0x55` through `0x100956A0`, applies zone and actor flag checks, then consults
`0x101CC4C0` / `0x10177750`. This trace does not establish all terrain and
mount-ID rules. Do not equate a chocobo-shaped model with digging permission.
For the server permission boundary, LandSandBoat's
`vendor/server/src/map/packets/c2s/0x01a_action.cpp`
`GP_CLI_COMMAND_ACTION::process` accepts ChocoboDig only for MOUNT_CHOCOBO.

Live retail interaction was unavailable: the observation doctor reported a
locked screen and no client window. Exact on-screen confirmation placement,
non-self interaction, and the remaining Dig predicates are not observed here.
