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
                     (mechanical drive loops → haiku subagent, see below)
4. Evidence      → JSON events / tracing log / screenshots, quoted in the report
5. Record it     → scripts/record-evidence.sh (feeds the stop-hook verify gate)
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

## Delegating the drive loop (cheap models)

Driving is mostly mechanical: long runs of small tool calls (MCP waits, stdio
commands, capture/read cycles) with little reasoning per step. Don't spend the
orchestrating model's time and context on that — spawn a subagent with the
Agent tool using `model: haiku` to run the loop, and keep the judgment work
(picking the surface, interpreting evidence, writing the report) here.

The driver's brief must be self-contained — it inherits none of your context:

- exact commands, not intent: paste the launch/attach/drive invocations from
  the reference file rather than making a small model re-derive them;
- the goal phrased observably ("stop when `zone_changed` fires with zone 231",
  not "zone the character");
- where to save artifacts (`artifacts/verify/…`) and what to return: artifact
  paths plus the observed event/log lines — never a bare "done";
- an escalation rule: if state is ambiguous, or two consecutive actions change
  nothing, stop and report what it saw — a confused driver that keeps sending
  commands destroys the scene you set up.

Interpretation stays here: read the returned artifacts yourself before citing
them as evidence. If the driving itself needs judgment (bisecting which layer
is broken, probing off the happy path), do it inline — delegation is for the
mechanical middle, not the thinking.

## Character strategy

- **Real character** (the user's) — credentials come from env
  (`FFXI_USER`/`FFXI_PASS`/`FFXI_CHAR`); never commit or log them.
- **Fresh provisioned chars** (`provision` + `create-char`, no DB teleport
  needed) — full E2E vehicle including zone changes and Mog House entry. The
  old "fresh chars get all c2s silently ignored" blocker was two client bugs,
  both fixed: the c2s datagram header must be the last subpacket's sync
  (`session.rs::datagram_header_id` — drift = server skips every subpacket
  silently), and the new-char intro cutscene rides the 0x00A LOGIN packet
  (`decode::ZoneInEvent`) and must be answered with 0x05B or the char sticks
  InEvent (0x05E/0x0E7 rejected). If those symptoms ever return, check the
  sync/header invariant first (`map_networking.cpp:419-428`).
- **Ephemeral fixture chars** (`ffxi-client/tests/common/mod.rs::EphemeralChar`)
  — created against the live lobby + DB, gmlevel set pre-first-login; what the
  integration tests use. Gotcha: when the accounts AUTO_INCREMENT outruns the
  fixture's sentinel accid scheme, the lobby rejects the char select
  ("mismatched character name" in connect logs) and the test dies at the 0x02
  ack step. Also rebuild `target/debug/ffxi-mcp` — the agent_session test
  spawns it without rebuilding.

## Reporting

The verdict is table stakes; observations are the signal. Quote the evidence
inline — event-stream lines, tracing log lines, map-server log lines,
screenshot paths — and keep the raw capture files (`events.jsonl`,
`client.log`) until the report is delivered. Anything driven around but not
observed (e.g. a GUI leg that needs human eyes) gets named explicitly rather
than silently skipped. Probes off the happy path (wrong zone, dead server,
double-send) are worth a line each even when they hold.

## Recording evidence (feeds the stop-hook gate)

The stop-hook verify gate (`.claude/hooks/stop.d/25-verify.sh`) blocks session
end when gated source (`*.rs`/`*.wgsl` outside tests/vendor) changed but no
fresh evidence exists. After delivering the report, record the session:

```
.claude/skills/verify/scripts/record-evidence.sh \
  --verdict pass --summary "<what was OBSERVED, one line>" \
  --artifact artifacts/verify/events.jsonl --artifact artifacts/verify/zone.png
```

Rules the recorder enforces: `pass` requires ≥1 artifact; artifacts must exist
non-empty; `--summary` records observations, not intentions. If runtime
verification genuinely doesn't apply, `--verdict waived --summary "<why>"` —
a waiver goes stale like any marker, so it only covers edits made before it.
The marker (`.verify/latest.json`) is stale the moment gated source is edited
after it; verify last, record last of all.

## Maintenance

When a verification session teaches something durable — a new env failure
mode, a better drive recipe, a lifted blocker — fold it into the matching
reference file in the same commit as the fix or finding. This skill rots
fastest at the env-gotcha layer.

## Subagent delegation (REQUIRED)

Verification runs (headless captures, GUI drives, retail comparisons) should run in subagents to keep the main context clean:

- `model: "haiku"` (haiku-4-5): mechanical evidence collection — run the documented commands, capture, read, report deltas.
- `model: "sonnet"` (sonnet-5): judgment passes — driving the client through menus, comparing local output against retail references, deciding pass/fail per criterion.
- Main agent receives only verdicts + evidence paths (artifacts/verify/...), not raw screenshot streams.
- Give subagents the exact scripts (`scripts/`, `references/drive-headless.md`, `references/drive-gui.md`) and the specific criteria to check; they should not improvise scope.
