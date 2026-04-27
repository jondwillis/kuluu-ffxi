---
name: diagnose
description: Print a structured report of why the FFXI agent is stuck (or confirm it isn't). Reads scene/party/diagnostics/goal/entities resources and the last-event sidecar, then names what's blocking action — empty entity list, zone=0, KO, missing party invite, etc. — without spending a play turn.
disable-model-invocation: true
---

# /diagnose — why is the agent stuck?

Operator-invoked diagnostic. Run this when the agent is looping
`path_to` perturbations or sitting on `Idle` and you want to know what
the perception layer is actually seeing.

## What to do when invoked

Execute these in parallel via the `ffxi` MCP server, then synthesize the
report below. Do **not** issue any movement, combat, or chat tool — this
is read-only.

1. `snapshot` (forces a fresh `SceneSummary`).
2. `read_resource { uri: "scene://current" }` (compact prose).
3. `read_resource { uri: "scene://entities" }` (structured nearest-N).
4. `read_resource { uri: "party://members" }`.
5. `read_resource { uri: "diagnostics://session" }`.
6. `read_resource { uri: "goal://current" }`.
7. `read_resource { uri: "inventory://current" }`.
8. Read sidecar: `~/.config/ffxi-mcp/last-event.json` (may not exist).

## Report format

Output a markdown report with these sections, each one short:

### Connection
- stage, blowfish_status, sync_in/sync_out delta from last sample,
  last_server_packet_age_ms. Flag if age > 5000ms.

### Character
- name, char_id, zone_id (with name if known — Phoenix zone IDs:
  100=W. Ronfaure, 102=W. Sarutabaruta, 230=Mhaura, etc.), main_job +
  level, HP/MP percent.

### Action surface
For each, say "available" or "blocked":
- **engage**: count of `entities[].kind == "mob"` with `claimed_by == null`
- **follow**: count of party members other than self
- **path_to**: do we have `self.pos` populated (any non-zero coord)
- **bank_when_full**: `inventory.all_loaded`?

### Active goal
- From `goal://current`. If `idle` and the action surface above is also
  empty, that's the diagnosis.

### Recent event
- From sidecar. Kind + age in ms. If age < 30s, that's likely the
  intended trigger for the next turn.

### Diagnosis (one paragraph)
Pick the most likely "stuck" cause and name it:
- "Zone has no engageable mobs in scene." → suggest `path_to` to
  another camp coord, or zone change.
- "Connection healthy but scene perception thin (zone_id=0)." → known
  pre-Stage-11 limitation; suggest moving the character to a real zone
  via the FFXI client UI.
- "KO'd at homepoint, no raise tool exposed." → operator must intervene
  in the client UI.
- "Idle goal + no events + no targets + no party." → the autonomy
  contract has nothing to chew on; suggest party invite or zone change.

### Suggested next action
A single concrete tool call the operator could make manually, or a
single sentence describing what state would unblock the loop.

## Don'ts

- Don't issue any tool that mutates state (`engage`, `path_to`,
  `cast`, `chat`, `disconnect`, `cancel`). Read-only.
- Don't speculate beyond what the resources show. If a section has no
  data, say "no data" — don't fabricate IDs or coords.
- Don't run `/diagnose` recursively. One pass is the whole skill.
