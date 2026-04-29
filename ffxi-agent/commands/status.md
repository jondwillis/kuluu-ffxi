---
description: Show the agent's current FFXI session state — zone, position, HP/MP, active goal, supervisor stage.
---

Read these MCP resources from the `ffxi` server and summarize:

1. `scene://current` — compact prose summary of the agent's surroundings.
2. `diagnostics://session` — JSON: stage (`Idle`/`Authenticating`/`Lobby`/`Map`/`InZone`), sync_in/out counters, packet age.
3. `goal://current` — JSON: the goal the supervisor will resume on reconnect (may be `null`).

Format the output as:

```
Stage: <stage>
Zone:  <zone name from scene>
Pos:   <x, y, z from scene>
HP:    <hp_pct>% MP: <mp_pct>%
Goal:  <follow|engage|path_to|null>
Sync:  in=<n> out=<n> age=<ms>ms
```

If the stage isn't `InZone`, just print the stage and any auth errors from
diagnostics — the rest is meaningless until the agent zones in.
