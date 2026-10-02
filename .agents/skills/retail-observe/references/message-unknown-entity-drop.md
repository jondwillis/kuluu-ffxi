# Message packets that reference unknown entities are dropped, not rendered (2026-09-18)

Static disassembly only. No Square Enix binary was executed, patched or
modified; no traffic was sent to any host. Question answered: when an s2c
message packet references an entity the client has not recorded (missed/raced
spawn over UDP), does retail render a placeholder name, defer the message,
request the entity, or drop it?

Build: the `retail` registered install, KNOWN_CLIENTS row `retail-2026-09`
(patch `30260904_1`). Unpacked `.text` SHA-256
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9` (matches the
row pin), first byte VA `0x10001000`. All addresses below are VAs on that
build. Dump kept local only.

## How the handler was found (reusable recipe)

The s2c dispatch tables are populated at runtime, so no static table scan
finds them. The sub-packet walk at `0x100FA716` extracts `id = word&0x1FF`,
`size = (word>>9)*4`, then calls through two tables on the object whose global
pointer is `0x104DFD94`: table1 at `object+0x44730`, table2 at
`object+0x442B4` (bounds `id < 0x11F`). Registration functions:
`0x100FC340` (table2) / `0x100FC370` (table1), behind thunks `0x100FC3A0` /
`0x100FC3B0`; each registration site pushes `(handler, id)`, so reading the
thunks' call sites maps every packet id to its handler VA.

Anchor that bootstrapped this: the Blowfish initial P-array (pi digits) at
`.data` VA `0x10360340`; its only code reference is the key-init at
`0x100DC710`; following callers up (`0x100DD520/0x100DD570/0x100DD590`
wrappers) reaches the packet crypto+I/O layer at `0x100F9xxx-0x100FAxxx`.

## RecvEmotionMes (s2c 0x05A) = VA `0x100A06E0`

The packet is **dropped in full** (returns 0; no chat text, no motion, no
re-request) when any of:

- `MesNum >= 97` (`0x100A06F4`).
- `CasActIndex >= 0x901` (bounds helper `0x10086FC0`, one instruction),
  the actor-table slot is null, or the slotted actor's `UniqueNo`
  (actor `+0x78`) != the packet's `CasUniqueNo` (`0x100A0709-0x100A0758`).
  The actor table is a pointer array at `0x10480AF0` indexed by ActIndex.
- `TarUniqueNo != 0` and the target fails the same three checks
  (`0x100A0759-0x100A07A4`).
- The emote DMsg line is missing (`0x100A07E1-0x100A07FE`; fetch is
  `0x100964A0(table=4, line)`).

Only then does it compose text (via `0x100A1770`/`0x100A1A00`), so the names
always come from validated actor structs — a "name unknown at compose time"
case does not exist in this handler. There is no `#%08X`-style placeholder,
no deferral until spawn, and no entity re-request from this path.

Cross-validations of existing kuluu code:

- DMsg line index is `MesNum*2 + (untargeted ? 1 : 0)` (`0x100A07DA-0x100A07E1`)
  — exactly `ffxi_dat::dmsg::emote_line_index`.
- `Mode == 1` (text-only) skips the motion path (`0x100A08F1`), matching the
  XiPackets mode table.
- The `/emotefaith` loop iterates exactly 5 (UniqueNo, ActIndex) pairs with
  per-entry null-skip (`0x100A09A0-0x100A0A95`).

## GP_SERV_COMMAND_BATTLE_MESSAGE (s2c 0x029) = VA `0x1009E8D0`

Same policy: both ActIndexes bounds-checked, both actor-table slots required
non-null, both UniqueNos must match the packet, and game state must be `0x60`
(in-game); any failure drops the message (`0x1009E8D0-0x1009E9E2`). No
placeholder text, no re-request.

## Where the healing actually lives: c2s 0x016 CHARREQ

Retail never repairs from a message handler. The recovery mechanism is on the
entity-streaming path:

- atom0s's XiPackets (`research/XiPackets/world/client/0x0016`): the client
  sends `GP_CLI_COMMAND_CHARREQ` (id `0x0016`, 8 bytes, carries `ActIndex`)
  "if it is attempting to access an entity it does not have valid data for
  yet (generally during events)"; the server answers with an entity update.
- LSB's handler (`vendor/server/src/map/packets/c2s/0x016_charreq.cpp`
  `GP_CLI_COMMAND_CHARREQ::process`) re-sends `ENTITY_SPAWN` with
  `UPDATE_ALL_CHAR`/`UPDATE_ALL_MOB` for the requested ActIndex (self-ActIndex
  gets own spawn + `CCharStatusPacket`; hidden GMs refused). `0x017_charreq2`
  is the follow-up variant.
- XIClient's reconstruction (tier 2, corroborates the trigger): in the s2c
  `0x00D`/`0x00E` update handlers, a non-complete update for an actor whose
  accumulated `SendFlg` is still 0 deletes partial state and sets the
  need-data flag (`research/XIClient/src/XIClient/source/Game/Net/Packets/s2c/0x00D.cpp:77`,
  `0x00E.cpp:170`); the actor's per-frame `Idle()` then sends `RequestChrData`
  and re-sends on a ~600-tick timer while unresolved
  (`.../World/Actor/ActorTelemetry.cpp:1799-1803` and the
  `AUDIT_1FE/AUDIT_1FC` block near the end of `Idle()`).

LSB broadcasts MOTIONMES with `CHAR_INRANGE_SELF`
(`vendor/server/src/map/packets/c2s/0x05d_motion.cpp` `process`), so both
entities are normally already streamed; an unknown entity means a lost or
reordered spawn datagram.

## Consequence for kuluu

Vanilla-faithful behavior for id-referenced message packets (emote, battle
messages) is: resolve via the ActIndex slot, require the UniqueNo to match,
and **silently drop** on failure — do not render a `#%08X` placeholder, do not
hold the line for flood-in. To make the failure rare rather than loud,
implement the client→server CHARREQ recovery on the update path. Open
question not covered here: whether the event-dialog string path (cutscene
speakers) requests CHARREQ, matching atom0s's "generally during events" note
— check before changing event dialog fallback rendering.
