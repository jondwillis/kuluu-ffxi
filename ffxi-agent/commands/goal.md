---
description: Set or clear the agent's high-level goal. Usage — /ffxi:goal follow <id>, /ffxi:goal engage <id>, /ffxi:goal path <x> <y> <z>, /ffxi:goal cancel.
---

Parse the user's argument as one of:

- `follow <target_id> [distance]` — call the `follow` tool on the `ffxi` MCP
  server. Default `distance` is 3.0 if not supplied.
- `engage <target_id>` — call the `engage` tool.
- `path <x> <y> <z>` — call the `path_to` tool with those coordinates.
- `cancel` — call the `cancel` tool.

Before issuing the goal, read `scene://current` to confirm the target_id (if
any) is actually present in the visible entity list. If not, ask the user to
re-check or pull a fresh snapshot via the `snapshot` tool.

After issuing the goal, read `goal://current` and confirm the supervisor has
persisted it correctly. Report the persisted goal back to the user.

Do not use this command to issue raw `move` packets — the reactor handles
per-tick movement once a goal is set. If the user genuinely wants manual
control they can call the `move` tool directly.
