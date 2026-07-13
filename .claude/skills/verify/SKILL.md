---
name: verify
description: >
  Runtime-verify Kuluu client changes against the local LSB dev stack — pick the
  right surface (headless MCP drive, GUI attach, raw stdio, or live integration
  tests), bring the stack up, drive the change, capture evidence. Use this
  whenever asked to verify a change works, confirm a bug fix, run/launch/drive
  the client, screenshot the game, test against the local server, or observe
  protocol/rendering behavior live — even if the word "verify" isn't used.
  Also the recipe for just getting a live session up for exploration.
---

# Verifying client changes against the live LSB stack

Verification here means **observing the change at a running surface** — a real
session against the real server — not re-running tests or reading code. The
work splits into four steps; each has a reference file with the exact recipes.

```
1. Stack up      → references/stack.md      (colima/docker bring-up + env gotchas)
2. Pick surface  → table below
3. Drive + observe → references/drive-headless.md | references/drive-gui.md
4. Evidence      → JSON events / tracing log / screenshots, quoted in the report
```

## Picking the surface

Match the surface to where the change is observable, not to what's easiest.
A wire decode fix is invisible in a screenshot; a camera fix is invisible in
an event stream.

| Change touches | Observable at | Drive with |
|---|---|---|
| Wire protocol, session state, reactor goals, zoning, chat, entity/spawn flow | JSON event stream + MCP resources | **Headless MCP** (preferred) or raw stdio — `references/drive-headless.md` |
| Rendering, HUD, camera, input-driven movement, materials, minimap, audio | Pixels/audio of the native window | **GUI + MCP attach** — `references/drive-gui.md` |
| Session/zone/MCP transport layers as a whole | Live integration tests (self-skip without a server) | `cargo test -p ffxi-client --test play_lifecycle` / `zone_change` / `agent_session` — see `references/drive-headless.md` §Tests |
| Both (e.g. a fix spanning session.rs and view_native) | Verify the protocol half headless first — it's cheaper and isolates failures — then the GUI half | both references |

Two constraints shape everything:

- **The reactor is not wired in `play --headless`** (main.rs spawns the bare
  session). Anything reactor-driven — zoneline auto-trigger, follow, engage,
  pathing — only runs under `ffxi-mcp` standalone (which spawns
  supervisor→reactor→session) or in the GUI. Raw stdio can still send explicit
  `AgentCommand`s like `request_zone_change`.
- **GUI input paths (WASD movement, camera) cannot be driven remotely.** The
  agent socket carries session-level `AgentCommand`s, not keystrokes. For those,
  set the scene up programmatically, then either capture screenshots externally
  or hand the window to the user for the input-driven part — say exactly which
  observations still need their eyes.

## Character strategy

- **Real character** (the user's) — the only reliable vehicle for zone-change /
  Mog House E2E today. Credentials come from env (`FFXI_USER`/`FFXI_PASS`/
  `FFXI_CHAR`); never commit or log them.
- **Ephemeral fixture chars** (`ffxi-client/tests/common/mod.rs::EphemeralChar`)
  — created against the live lobby + DB, gmlevel set pre-first-login; what the
  integration tests use. Fine for login, spawn-stream, inventory observation.
- **Known blocker**: manually provisioned fresh chars (lobby `create-char` +
  DB pos teleport) get all substantive c2s **silently ignored** by the server
  (0x05E zonelines, 0x0B5 chat — no response, no echo, no server log) while
  keepalive/entity flow stays healthy. Reproduced across client commits, so
  it's server-side char/account state (suspect: unfired new-char intro
  cutscene). Don't burn time re-diagnosing it mid-verification; fall back to a
  real char and note the gap.

## Reporting

The verdict is table stakes; observations are the signal. Quote the evidence
inline — event-stream lines, tracing log lines, map-server log lines,
screenshot paths — and keep the raw capture files (`events.jsonl`,
`client.log`) until the report is delivered. Anything driven around but not
observed (e.g. a GUI leg that needs human eyes) gets named explicitly rather
than silently skipped. Probes off the happy path (wrong zone, dead server,
double-send) are worth a line each even when they hold.

## Maintenance

When a verification session teaches something durable — a new env failure
mode, a better drive recipe, a lifted blocker — fold it into the matching
reference file in the same commit as the fix or finding. This skill rots
fastest at the env-gotcha layer.
