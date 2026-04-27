---
name: lsb-mirror-check
description: For any Rust symbol that crosses the LSB boundary (wire decoder, packet builder, coord transform, session-state transition, shared constant, lifecycle assumption), locate and surface the LSB C++ counterpart in vendor/server/ for side-by-side comparison. Use whenever implementing or modifying anything whose correctness depends on server-side semantics matching.
disable-model-invocation: true
---

# lsb-mirror-check

Invoke as `/lsb-mirror-check <rust_symbol>` (or describe the symbol). The
mechanism is generic over boundary types — the rules below tell you
**where to look** and **what to verify**, regardless of which symbol.

## When this applies

A Rust symbol "crosses the LSB boundary" if its correctness depends on
some piece of the LSB server implementation. Common boundary types in
this codebase:

| Boundary type | Examples | Where LSB defines truth |
|---|---|---|
| Wire packet decoder | `PosHead::decode`, `ServerLogout::decode` | `vendor/server/src/map/packets/s2c/0x*.{h,cpp}` |
| Wire packet builder | `build_subpacket_*`, `build_bootstrap_packet` | `vendor/server/src/map/packets/c2s/0x*.{h,cpp}` |
| Coord transform | `ffxi_to_detour`, `ffxi_to_bevy`, the build.rs y/z swap | `vendor/server/src/map/navmesh.cpp` (`ToDetourPos`/`ToFFXIPos`), `vendor/server/src/common/mmofile.h` (position_t layout) |
| Session-state transition | reconnect path, key rotation, status machine | `vendor/server/src/map/map_networking.cpp`, `vendor/server/src/map/map_session.cpp` |
| Numeric constant | message IDs, opcodes, status enums | `vendor/server/src/map/enums/*.h`, `vendor/server/sql/*.sql` |
| Lifecycle assumption | when sessions are created/destroyed, IPC ordering | `vendor/server/src/map/map_session_container.cpp`, `vendor/server/src/map/ipc_*.cpp` |
| Action handler | `0x01A` action sub-IDs, packet validation | `vendor/server/src/map/packets/c2s/0x01a_action.cpp`, `Phoenix/src/.../0x01a_action.cpp` |
| Battle message template | placeholder→slot binding per `messageId` | `vendor/server/src/map/enums/msg_basic.h` + emission call sites |

If the Rust symbol doesn't appear in this table's "Examples" column,
ask: does its behavior have to match what the server does, or can it
be arbitrary? If the former, it's a boundary symbol — proceed.

## Lookup procedure

1. **Generate name candidates.** LSB's C++ uses these naming
   conventions; try all that fit the symbol's role:
   - PascalCase functions on classes: `CNavMesh::ToDetourPos`,
     `CCharEntity::Raise`
   - Phoenix-style `GP_CLI_COMMAND_*` / `GP_SERV_COMMAND_*` for packet
     structs (used in both LSB and Phoenix headers)
   - `MsgBasic::PlayerDefeatedBy` for enum-named message IDs
   - `charutils::SendToZone`, `zoneutils::GetZoneIPP` for utility
     functions (look in `src/map/utils/`)
   - Opcode hex: `0x05E`, `0x01A` — file names follow this exactly
   - Lowercase variants if the function predates the project's
     migration to PascalCase (rare)

2. **Grep both vendored trees.**
   ```
   grep -rn '<candidate>' vendor/server/src/
   grep -rn '<candidate>' vendor/Phoenix/src/
   ```
   The running container uses **LSB** (per
   `~/.claude/projects/.../memory/server_is_lsb_not_phoenix.md`), so
   LSB is authoritative for runtime. Phoenix is a useful divergence
   signal — if it has rewritten the function, that's a clue the area
   has known protocol risk.

3. **Read both implementations.** Surface them side-by-side with
   file:line citations. Compare:
   - Byte offsets, field widths, signedness, endianness
   - Sign conventions on shared physical axes
   - Numeric values of constants
   - State transitions (especially: when does the session change
     status? when is `PChar` reset?)
   - Return-value semantics (LSB sometimes returns early; mirror
     that)

4. **Produce a verification checklist scoped to the boundary type.**

## Boundary-type checklists

### Wire packet (decoder or builder)

- [ ] Body length matches LSB's `sizeof(struct)` (account for
      pre-PS2-era variants — LSB tolerates short bodies for some)
- [ ] Each field's byte offset matches LSB's layout (mind padding /
      alignment in the C struct)
- [ ] Signedness and integer width match (`uint8_t LogoutState` is
      one byte; a u32 read happens to work only if the next three
      bytes are zero padding)
- [ ] Endianness: FFXI is little-endian on wire
- [ ] Field-order on **send** matches what server's `recv_parse`
      expects, even when LSB's struct field order differs from the
      wire byte order (`build_subpacket_pos` example: wire is
      `(x, height, north)` but LSB's `position_t` is read field-by-
      field, so order matters)

### Coord transform

- [ ] Round-trip: `inverse(forward(v)) == v` for arbitrary v
- [ ] Sign convention on shared axes matches LSB's
      `CNavMesh::ToDetourPos` / `ToFFXIPos` (height and north axes
      negate in LSB's convention)
- [ ] If multiple transforms exist (e.g., FFXI→Detour, FFXI→Bevy,
      Detour→Bevy), they form a commutative diagram:
      `D→B == ffxi_to_bevy ∘ detour_to_ffxi`
- [ ] Unit test pinning each axis individually (a "pure-height"
      vector, a "pure-north" vector) so a future drive-by edit that
      drops a sign trips a test

### Numeric constant / enum

- [ ] Value matches LSB's definition (`vendor/server/src/map/enums/`)
- [ ] Prefer a `build.rs` scraper over hand-maintained constants
      where the LSB definition is in a parseable header or SQL file
      (the `ffxi-proto/build.rs` and `ffxi-nav/build.rs` patterns
      are the precedents). Hand-maintained values drift.

### Session-state transition

- [ ] Triggering conditions match: what packet/event makes LSB
      transition? Does Rust transition on the same trigger?
- [ ] Order of operations matches: e.g., LSB rotates blowfish key
      **after** sending 0x00B, then writes the new key to DB. Rust
      must rotate seed only after **receiving** the 0x00B, before
      reconnecting.
- [ ] Lookup keys match: e.g., LSB looks sessions up by `(ip, port)`
      tuple; if Rust changes the source port mid-session, LSB can't
      find the session (catches the rebind-vs-retarget bug class).
- [ ] All status enums LSB uses are decoded in Rust (e.g.,
      `BLOWFISH_WAITING` vs `BLOWFISH_PENDING_ZONE` vs `BLOWFISH_ACCEPTED`)

### Battle-message / placeholder binding

- [ ] Each message's `<placeholder>` → wire slot mapping verified
      against the Phoenix/LSB call site that emits it. The
      placeholder name in `msg_basic.h` comments is **not** a
      reliable slot indicator — different messages bind `<player>`
      to different slots depending on grammatical role.

## After running this check

Cite LSB findings in code comments at the point of the boundary:

```rust
// LSB's `CNavMesh::ToDetourPos` (vendor/server/src/map/navmesh.cpp:141)
// negates both height (FFXI y) and north (FFXI z); we mirror.
```

These citations are not decoration — they're consumed by the
PreToolUse hook (`.claude/hooks/lsb-boundary-reminder.sh`) to detect
which files cross the boundary. A file that needs LSB-check but
lacks a citation is one of the issues this skill is supposed to
catch: surface that "missing citation" gap as a finding too.
