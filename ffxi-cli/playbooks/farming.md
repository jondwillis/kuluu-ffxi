# Playbook: 60-minute solo farming run

Concrete instructions for a single-character farming session. Goal:
sustained engage-loop on a starter mob with automatic banking and
disconnect recovery. Target run length: 60 minutes including one
forced disconnect mid-run.

## Pre-run checklist

The harness needs three pieces of information that vary per character
and per run. Ask the user for these — don't guess:

| Field | Example | Source |
|---|---|---|
| Farming zone id | 115 (West Sarutabaruta) | `scene://current` shows it after zone-in |
| Target mob ids | varies — pick from the prose summary | `scene://current` |
| Mog-house zoneline RectID | varies per starting city | the user's setup |

Default banking threshold: 60 (leaves headroom for drops; Inventory
maxes at 80, sometimes less depending on bag upgrades). 90 is "fight
until almost full"; 30 is "very conservative."

## Loop

1. **Initial setup.** Once at the start of the run:

   ```
   bank_when_full { threshold: 60, mog_house_zoneline: <line_id> }
   ```

   The reactor will hold the goal across the entire session and
   one-shot a zone-change when triggered. After zoning back from
   the mog house, re-arm with another `bank_when_full` call —
   the goal clears once it fires.

2. **Each combat cycle.** Loop until the user calls disconnect:

   1. `snapshot` → re-read `scene://current`. Identify a target mob from
      the prose. Pick targets that match the agent's level / job;
      starter mobs (lvl 1–10) are the safest defaults at the start.
   2. `engage { target_id: <id> }`.
   3. Wait for either:
      - `low_hp` event on self (HP < 25%) → `cancel`, retreat with
        `path_to` to a safe spot, sleep until HP regenerates.
      - The mob despawns (visible by re-reading `scene://current`
        and finding the target id absent).
   4. On mob death: `cancel`, then go back to step 1.

3. **Disconnect handling.** Do nothing. The supervisor reconnects
   automatically with exponential backoff. The persisted
   `goal://current` resource replays the last set goal once the
   session is back in-zone — banking continues, engage-loop
   resumes from a fresh `scene://current` read.

## Banking trip details

When `bank_when_full` fires its `RequestZoneChange`:

- Character zones into mog house.
- Fresh inventory packets flood in over a few hundred ms; once
  `0x01D ITEM_SAME { state: AllLoaded }` arrives, `inventory://current`
  reports `all_loaded == true`.
- The harness should:
  1. Read `inventory://current` → list of slots in the field bags
     that are eligible for storage.
  2. Walk to the mog locker NPC (`path_to` with the NPC's coordinates,
     which the user provides in the per-character setup).
  3. Manual storage moves are **not** in the v1 tool surface — flag
     this gap and let the user handle the actual transfer until a
     `move_item` tool ships.
  4. After the user confirms transfer, `request_zone_change` back
     to the farming zone.
  5. Re-arm `bank_when_full` for the next cycle.

This is the largest gap in the agent surface today: the
`bank_when_full` tool gets the character to the mog house but
cannot unload the bags. The 60-minute run completes one banking
trip with manual help.

## Failure modes

| Symptom | Likely cause | What to do |
|---|---|---|
| `path_to` returns "ok" but agent doesn't move | Out-of-bounds reject by the server — happens silently at the wire | Re-read `scene://current`, pick a target inside the visible mesh. |
| Engage holds but no damage | Mob despawned (other player tagged it) | `cancel`, pick a fresh target. |
| Sustained 200ms+ packet age in `diagnostics://session` | Server lag or proxy drop | Wait one tick, re-snapshot. If persistent, supervisor will reconnect on its own. |
| Character dead (`hp == 0`) | Aggro chain you didn't see | `cancel`, await raise (manual today; no auto-raise tool). |

## Pass criteria for the test run

The forward plan's Stage-12 acceptance for the farming run:

1. Run length ≥ 60 minutes.
2. At least one supervisor `Reconnected` event mid-run (artificially
   triggered: `docker restart phoenix` from another terminal).
3. Goal replay observable in `goal://current` immediately after
   the reconnect — `bank_when_full` should re-appear with the
   same threshold.
4. Capture `artifacts/farming-run-1.jsonl` from the agent stdout
   (event stream) for inspection.
