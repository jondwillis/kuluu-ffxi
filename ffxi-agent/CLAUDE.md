# FFXI agent harness — Claude Code playbook

This repo runs an LLM-driven agent on a LandSandBoat-family Final Fantasy XI
server (Phoenix dev container or HorizonXI / Phoenix-launch live). Your role
is the **strategy layer**: a 200 ms-tick deterministic reactor in Rust handles
keepalive, follow-target, auto-attack, and event auto-dismiss. You drive
high-level intent through the MCP `ffxi` server.

## How the loop works

```
you (LLM)
  │  MCP/stdio
  ▼
ffxi-mcp ──cmd_tx──▶ supervisor → reactor → session → FFXI server
   │                     │           │
   └─events ─────────────┤           │ 200ms tick
                         │           │
                  ~/.config/ffxi-mcp/goal.json  ← persisted
```

You wake up on:

1. **Goal change** — you decide a new high-level intent.
2. **`tell` / `low_hp` / `engaged_by`** — high-signal events the reactor
   surfaces. Re-fetch `scene://current` and decide.
3. **Periodic check** — re-pull `scene://current` every ~5 s when active.

You **do not** drive per-tick movement. Issuing `move` overrides the reactor
and clears the goal — only do that when you genuinely want manual control.

## Tools (in order of how often you'll use them)

| Tool | When to use |
|---|---|
| `follow { target_id, distance }` | Stick to a party leader (co-play) or chase a mob. Reactor handles per-tick steps. |
| `engage { target_id }` | Begin auto-attack on a mob. Reactor sends one Attack action then keeps facing. |
| `path_to { x, y, z }` | Walk to specific coordinates. Navmesh-aware (Stage 10b): emits a waypoint list when a 2D grid is available, falls back to a single straight segment otherwise. Cliffs / vertical drops are not yet modelled. Out-of-bounds steps are rejected by the server. |
| `cancel` | Clear active goal, return to Idle. Also clears persisted goal on disk. |
| `chat { kind, text }` | `kind`: 0=say, 1=shout, 4=party, 5=linkshell. /tell support requires a target field that v1 doesn't have. |
| `request_zone_change { line_id }` | Trigger a zoneline. The character must already be standing in the zoneline rect. |
| `end_event` | Dismiss any in-progress NPC event/cutscene. Cheap; safe when no event is active. |
| `snapshot` | Force-emit a fresh `SceneSummary` event and `Diagnostics`. Triggers re-fetch of `scene://current`. |
| `cast { spell_id, target_id, target_index, pos_x?, pos_y?, pos_z? }` | Cast a spell by FFXI Spells.dat id. Self-target casts pass own UniqueNo+ActIndex; ground-target spells (Tractor) supply pos_*. |
| `weaponskill { skill_id, target_id, target_index }` | Use an unlocked weaponskill. Server validates TP / weapon. |
| `job_ability { ability_id, target_id, target_index }` | Use a job ability (e.g. WAR Mighty Strikes, RDM Convert). Server validates cooldown / job. |
| `use_item { container, slot, item_no, target_id, target_index }` | Use a consumable / scroll / charged item. `(container, slot)` identify the item; `target` is self for potions or another entity for ranged items (Soultrapper). |
| `disconnect` | Clean exit. Supervisor will not reconnect. |

## Resources

| Resource | Read when |
|---|---|
| `scene://current` | Always read first — compact prose summary. ~150 tokens. |
| `party://members` | When co-playing: HP/MP/TP/job per member. JSON. |
| `diagnostics://session` | Debugging: stage, sync_in/out, packet age. JSON. |
| `goal://current` | What goal the supervisor will resume on reconnect. JSON. |
| `inventory://current` | Container-keyed slot map. JSON. Read before `bank_when_full` so the threshold is sane for the active bag's capacity. |

`inventory://current` is shaped `{ containers: { <id>: { capacity, slots: [...] } }, all_loaded: bool }`. Container ids match Phoenix's `CONTAINER_ID` (0=Inventory, 1=Safe, 5=MogSatchel, 6=MogSack, 7=MogCase, 8=Wardrobe, …). The inventory floods in across many packets after a zone-in; `all_loaded` flips true once `0x01D ITEM_SAME { state: AllLoaded }` arrives. `bank_when_full` waits for that flag before trusting slot counts, so a thresholded goal set immediately after zone-in is safe — the reactor parks until the flood drains.

## Playbook: solo combat

1. Read `scene://current` to find your zone and what's nearby.
2. Identify a target mob from the prose summary.
3. `engage { target_id }` — reactor closes range, faces, auto-attacks.
4. Watch for `low_hp` notification → if HP < 25 %, `cancel` and retreat
   via `path_to` to a safe position.
5. On mob death, `cancel` then re-read `scene://current` for the next target.

## Playbook: 60-min farming

For an unattended grind in a single zone:

1. Read `scene://current` to confirm zone + nearby targets, and
   `inventory://current` to confirm `all_loaded: true`.
2. **Once at the start**: `bank_when_full { threshold: 60, mog_house_zoneline: <city zoneline RectID> }`.
   60 leaves room (an 80-slot Inventory still has 20 free); 90 is closer
   to "almost full". One-shot — re-issue after each banking trip.
3. Pick a target id from the prose. `engage { target_id }`.
4. Wait for the mob's death. The reactor fires `LowHp` when *your* HP
   crosses the threshold, and `EngagedBy` clears once the mob dies (the
   server stops broadcasting it as your battle target). If you'd rather
   poll, re-read `scene://current` every few seconds — the engaged
   target disappears from the prose when it's dead.
5. `cancel` to drop combat state, then re-read `scene://current` for
   the next target. Repeat from step 3.
6. Use `cast` / `weaponskill` / `job_ability` mid-fight as needed:
   `cast { spell_id, target_id, target_index }` for nukes/cures (target
   self via your own UniqueNo + ActIndex), `weaponskill` once TP ≥ 100,
   `job_ability` for cooldown abilities. Server validates MP / TP /
   cooldown / job — failures come back as chat lines, not exceptions.
7. Use `use_item { container, slot, item_no, target_id, target_index }`
   for potions/ethers/scrolls; `(container, slot)` is what the server
   resolves the item by, `item_no` is the LLM's bookkeeping hint.

Disconnects are the supervisor's problem. Goals persist to
`~/.config/ffxi-mcp/goal.json`, so a reconnected session resumes
whatever was last active — `Engage`, `Banking`, etc. You don't need to
re-issue them on `Reconnected`; just re-read `scene://current` to
re-orient.

## Playbook: co-play (party member)

1. `follow { target_id: <leader_id>, distance: 3.0 }` — stick to the leader.
2. Subscribe to `party_member_low_hp` notifications.
3. On `tell` containing "@cure" / "@heal", read `scene://current`, find the
   sender, decide to cast a healing spell.
4. Subscribe to `engaged_by` → you got aggro; the leader will tank or you
   need to kite.

## Don'ts

- **Don't** issue per-tick `move`. The reactor's job. Manual `move` clears
  the goal — only do it for genuine override.
- **Don't** assume the agent's target is current. Re-pull `scene://current`
  when the situation changes.
- **Don't** invent opcodes or packet shapes. Tools cover the safe set; if
  you need a new action, ask for a new tool, don't try to send raw bytes.
- **Don't** call `disconnect` to "restart" — the supervisor won't reconnect
  after a deliberate disconnect. Use it only when you genuinely want to quit.

## Configuration

Set credentials via env before launching Claude Code (or `.env` if your
harness reads one):

```bash
export FFXI_USER=...
export FFXI_PASS=...
export FFXI_CHAR_ID=...   # u32, from accounts_chars.charid
export FFXI_CHAR=...      # exact display name
export FFXI_SERVER=127.0.0.1   # or HorizonXI hostname
```

Then `.mcp.json` in this directory points Claude Code at `cargo run -p
ffxi-mcp`. First launch will compile; subsequent launches are instant.

## Live calibration caveats

Some pieces have **not** been validated against a live server:

- Heading math (`reactor::heading_toward`) — internally consistent
  (north=0/east=64/south=128/west=192) but may need a constant offset to
  match server expectations.
- `RequestZoneChange` — packet builder is unit-tested but the server-side
  acceptance has not been observed yet.
- Party-packet decode (`0x0DD` / `0x0DF`) — schema-checked but real
  party traffic has not exercised the merge logic yet.

If you see sustained "no movement" or zonelines refusing, ask the user to
run a calibration session and check Phoenix's `map.log`.

## Navmesh status

`path_to` is navmesh-aware (Stage 10b shipped) but the data plumbing is
partial. The reactor's loader tries, in order:

1. **Detour `.nav` binary** at `<server|Phoenix>/navmeshes/<zone_id>.nav`
   (the LSB / Phoenix submodule layout, walking up from the working
   directory). The Rust reader for the Recast/Detour binary format is
   **not yet implemented** — Stage 10c, deferred. Today the loader logs
   "Detour .nav file present but loader not yet implemented" and falls
   through.
2. **PNG occupancy heightmap** at
   `~/.config/ffxi-mcp/heightmaps/<zone_id>.png`. A best-effort 2D grid;
   hand-traced PNGs work for known farming zones. No vertical
   information — cliffs and elevation drops aren't modelled.
3. **Straight-line** segment otherwise.

So for any zone without a hand-drawn PNG, `path_to` is straight-line
today. Cliff-aware paths require Stage 10c (the Detour reader). When
you set a `path_to`, the resulting `Goal::Pathing` in the reactor
carries a waypoint list; `goal://current` shows the destination plus
`waypoints_remaining`, so you can tell at a glance whether the agent is
on a multi-segment route or one straight line.
