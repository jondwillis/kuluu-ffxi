# FFXI agent harness — playbook

You drive an LLM-controlled agent on a LandSandBoat-family Final Fantasy XI
server (Phoenix dev container or HorizonXI-class live). Your role is the
**strategy layer**: a 200ms-tick deterministic reactor in Rust handles
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
3. **Periodic check** — re-pull `scene://current` every ~5s when active.

You **do not** drive per-tick movement. Issuing `move` overrides the reactor
and clears the goal — only do that when you genuinely want manual control.

## Tools (in order of how often you'll use them)

| Tool | When to use |
|---|---|
| `follow { target_id, distance }` | Stick to a party leader (co-play) or chase a mob. Reactor handles per-tick steps. |
| `engage { target_id }` | Begin auto-attack on a mob. Reactor sends one Attack action then keeps facing. |
| `path_to { x, y, z }` | Walk to specific coordinates. Single straight segment; out-of-bounds rejected by server. |
| `cancel` | Clear active goal, return to Idle. Also clears persisted goal on disk. |
| `chat { kind, text }` | `kind`: 0=say, 1=shout, 4=party, 5=linkshell. /tell support requires a target field that v1 doesn't have. |
| `request_zone_change { line_id }` | Trigger a zoneline. The character must already be standing in the zoneline rect. |
| `end_event` | Dismiss any in-progress NPC event/cutscene. Cheap; safe when no event is active. |
| `snapshot` | Force-emit a fresh `SceneSummary` event and `Diagnostics`. Triggers re-fetch of `scene://current`. |
| `disconnect` | Clean exit. Supervisor will not reconnect. |

## Resources

| Resource | Read when |
|---|---|
| `scene://current` | Always read first — compact prose summary. ~150 tokens. |
| `party://members` | When co-playing: HP/MP/TP/job per member. JSON. |
| `diagnostics://session` | Debugging: stage, sync_in/out, packet age. JSON. |
| `goal://current` | What goal the supervisor will resume on reconnect. JSON. |

## Playbook: solo combat

1. Read `scene://current` to find your zone and what's nearby.
2. Identify a target mob from the prose summary.
3. `engage { target_id }` — reactor closes range, faces, auto-attacks.
4. Watch for `low_hp` notification → if HP < 25 %, `cancel` and retreat
   via `path_to` to a safe position.
5. On mob death, `cancel` then re-read `scene://current` for the next target.

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

Set credentials via env before launching the harness:

```bash
export FFXI_USER=...
export FFXI_PASS=...
export FFXI_CHAR_ID=...   # u32, from accounts_chars.charid
export FFXI_CHAR=...      # exact display name
export FFXI_SERVER=127.0.0.1   # or HorizonXI hostname
```

The MCP server invocation expects `ffxi-mcp` on PATH. Install with:

```bash
cargo install --path ffxi-mcp
```

## Live calibration caveats

Some pieces have **not** been validated against a live server:

- Heading math (`reactor::heading_toward`) — internally consistent
  (north=0/east=64/south=128/west=192) but may need a constant offset to
  match server expectations.
- `RequestZoneChange` — packet builder is unit-tested but the server-side
  acceptance has not been observed yet.
- Party-packet decode (`0x0DD` / `0x0DF`) — schema-checked but real
  party traffic has not exercised the merge logic yet.

If you see sustained "no movement" or zonelines refusing, ask the operator
to run a calibration session and check Phoenix's `map.log`.
