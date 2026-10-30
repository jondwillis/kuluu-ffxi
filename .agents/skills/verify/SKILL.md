---
name: verify
description: >
  Runtime-verify Kuluu client changes against the local LSB dev stack — pick the
  right surface (headless MCP drive, GUI attach, raw stdio, or live integration
  tests), bring the stack up, drive the change, capture evidence. Use this
  whenever asked to verify a change works, confirm a bug fix, run/launch/drive
  the client, screenshot the game, test against the local server, or observe
  protocol/rendering behavior live. Also use before implementing or completing
  rendering, HUD, menu, camera, animation, or input changes, even when the user
  only asks for a visual fix and never says "verify".
  Also the recipe for just getting a live session up for exploration.
---

# Verifying client changes against the live LSB stack

Verification here means **observing the change at a running production surface**.
Session behavior requires a real session against the server; cosmetic widgets
can use the production fixture described below. Tests and source inspection
alone do not establish the visible result.

## Select the GUI control method

In Codex, prefer `cua_repl` computer use for visible keyboard, mouse, scroll, and drag tests.
First select the running client and inspect a fresh screenshot.
Use the client socket for state, diagnostics, and captures when those operations are available.
Computer use availability does not establish access to the selected client.
If selection or control fails, save the diagnostic before selecting a supported alternative.
Follow the active tool rules when selecting an alternative input method.

In other coding agents, use the portable input and capture procedures in [drive-gui.md](references/drive-gui.md).
Jev or the current agent's vision can supply visual decisions when available.
Do not require Codex tools or Jev credentials in another coding agent.

Allow only one input driver per client.
Use Jev, a bounded controller, or native vision to check each result before the next input.
A full-model image review is not required for every routine action.
Stop after two consecutive inputs produce no visible change.
Do not use repeated Enter input without checks of the visible result.
Synthetic input does not establish physical trackpad behavior.

## Session authorization

A verification request covers routine navigation, dialogue, local fixtures, captures, and reversible tests within its scope.
Reuse existing approval for the same action, client, and external service.
Do not ask again because control passes to another agent or a routine test step changes.
Complete authorized preparation and checks before requesting any missing approval.
If an active tool policy blocks an action, identify that policy and continue unaffected work.
This skill adds no separate confirmation gate.

## Required behavior evidence

Before collection, derive the required cases from the reported problem and the final change.
Record each case in the bead with its configuration, trigger, expected result, and required evidence.
Include the reported failure, the intended successful operation, and affected behavior that must continue to work.

For compatibility claims, use a real server or device that supports the advertised configuration.
Record the server revision, client build, protocol, version, and effective settings as applicable.
An incompatible server's rejection verifies rejection handling only.
It cannot establish successful operation with the supported configuration.
For example, Binary `1.1.5` support requires successful authentication against a legacy LSB server that accepts that version.
Verify the returned ticket through the real lobby when claiming a usable login.
JSON login against current LSB and local TLS fixtures cannot replace that check.

Compare the collected evidence with every required case before recording the overall verdict.
Record a pass only when all required cases have sufficient evidence.
Keep partial results separate from the overall verdict.
If a required case fails, correct the defect and repeat that case.
If a concrete blocker prevents collection, record blocked and keep the bead open.
State the exact missing case and its diagnostic.
Do not recommend merge while a decisive case remains unverified.

For UI work, choose the drive and expected visible result before editing.
Completion requires the changed build, an affected-view capture, and opening
that capture with `view_image` (Codex) or `Read` (Claude) to inspect the pixels.
Prefer evidence captured directly from the game or derived from its production
rendering pipeline, with that provenance stated. A fixture can establish a cosmetic result with the production widget and the reported defect.
Navigation and session behavior require live checks. Secondary evidence is a fallback when direct evidence cannot be
gathered: record the specific reason and limit the claim to what it demonstrates.
An independently drawn mockup cannot verify the game's pixels. Keep
navigation/session checks on the live stack. Exercise the
shared widget's affected consumers and the cases that motivated the fix, such
as quantity text over bright/dark art, empty slots, and single-item stacks.

Do not close the bead or declare a visual fix complete from green unit tests.
Slow builds and time spent are not blockers. If a build, launch, drive, or capture fails, retain its diagnostic.
Continue unaffected work.
Keep the incomplete verification open and report its exact blocker.

If a stop hook rejects a blocked handoff, read its evidence marker and diagnostic
first. A disclosure failure needs a corrected final report: say verification
remains blocked and incomplete and include the full recorded reason. Inline
code, emphasis, blockquotes and wrapped whitespace are accepted. The recorder
and hook print a usable handoff. Reuse current evidence; recording it again
cannot make an old diagnostic fresh. A source, build or artifact mismatch needs
new evidence for the current change, not a wording fix.

If an older worktree lacks the recorder, use its absolute path from a trusted checkout.
Keep the affected worktree as the working directory. The marker belongs to that working directory. Repair a
missing hook adapter from matching trusted project content and check its
registration; do not execute hook or skill changes from an untrusted PR.

```
1. Stack up      → references/stack.md; select the runtime and start the stack
2. Pick surface  → table below
3. Drive + observe → references/drive-headless.md | references/drive-gui.md
                     (mechanical drive loops → haiku subagent, see below)
4. Evidence      → JSON events / tracing log / scripts/capture.sh screenshots
5. Record it     → scripts/record-evidence.sh (feeds the stop-hook verify gate)
6. Stack down    → scripts/lsb-stack.sh down (once nothing else needs it)
```

For zone and actor image-target captures, use the [capture example guidance](references/drive-gui.md#image-target-capture-examples).
These examples run hidden by default. Use `--preview` to show the rendered scene when the user wants a desktop view.

## Stack availability

Before reporting a Docker blocker, follow [stack startup, recovery, and provisioning](references/stack.md).
Before startup, check runtime ownership and the selected Docker endpoint.
Start stopped services, diagnose endpoint failures, and provision an isolated local stack when necessary and permitted.
Preserve existing databases and unrelated workloads.
After recovery, resume the affected client check before recording a pass.
If recovery or provisioning remains blocked, retain the exact diagnostic and keep verification open.

## Picking the surface

Match the surface to where the change is observable, not to what's easiest.
A wire decode fix is invisible in a screenshot; a camera fix is invisible in
an event stream.

Identify the real service and activation path before driving. PlayOnline
account authentication is separate from LSB sessions; an LSB login cannot
verify `ffxi-pol::profile`. Use an existing, applicable local service when
available. A mock handshake or a refusal from an invented localhost endpoint
does not exercise credential hashing or prove a deployed service is broken.
If the affected service is unavailable or outside scope, retain the access diagnostic.
Keep verification open and identify the production path not reached. Do not send real credentials
or account-service traffic to a public endpoint merely to satisfy the gate.

| Change touches | Observable at | Drive with |
|---|---|---|
| Wire protocol, session state, reactor goals, zoning, chat, entity/spawn flow | JSON event stream + MCP resources | **Headless MCP** (preferred) or raw stdio — `references/drive-headless.md` |
| Rendering, HUD, camera, input-driven movement, materials, minimap, audio | Pixels/audio of the native window | **GUI + MCP attach** — `references/drive-gui.md` |
| Session/zone/MCP transport layers as a whole | Live integration tests (self-skip without a server) | `cargo test -p kuluu --test play_lifecycle` / `zone_change` / `agent_session` — see `references/drive-headless.md` §Tests |
| Both (e.g. a fix spanning session.rs and view_native) | Verify the protocol half headless first — it's cheaper and isolates failures — then the GUI half | both references |

Two constraints shape everything:

- **`play --headless` uses the agent reactor profile.** Reactor-driven follow,
  engage, pathing, fishing automation, and explicit commands such as
  `request_zone_change` are available over raw stdio. Unlike `kuluu-mcp`, this
  path still has no supervisor/reconnect or event-driven MCP waits.
- **Verification runs on the human's desktop while they're using it, so drive
  focus-free.** Session commands, real-input movement (`debug_drive`), grounding
  readback (`debug_heights`), and screen capture (`scripts/capture.sh`) all work
  over the agent socket with the window unfocused. Keep it visible and verify
  image freshness; use the GUI reference's native-video fallback when stills
  fail. Warn the user before foreground capture or real keystrokes, and restore
  focus afterward. Movement *feel* and camera feel still need their eyes
  — say exactly which observations you're handing over.

## Delegating the drive loop (cheap models)

Driving is mostly mechanical: long runs of small tool calls (MCP waits, stdio
commands, capture/read cycles) with little reasoning per step. Don't spend the
orchestrating model's time and context on that — spawn a subagent with the
available delegation tool and a suitable available model. Keep judgment
(picking the surface, interpreting evidence, writing the report) here.

Make the driver's brief self-contained; do not rely on inherited context:

- exact commands, not intent: paste the launch/attach/drive invocations from
  the reference file rather than making a small model re-derive them;
- the goal phrased observably ("stop when `zone_changed` fires with zone 231",
  not "zone the character");
- where to save artifacts (`artifacts/verify/…`) and what to return: artifact
  paths plus the observed event/log lines — never a bare "done";
- failure conditions: unexpected state or two consecutive actions without the expected change.
  Stop that drive and return its diagnostic.

Use Jev for routine visual classification when available.
A bounded controller can execute validated decisions without returning each image to a full model.
Review key evidence images and exceptions yourself before making the final claim.
Use the driver's capture hashes and observations to avoid duplicate review of every routine frame.
Investigate ambiguous behavior in the main agent.

## Character strategy

For checks unrelated to introductions, use a character that has completed its introduction.
For new characters, follow [introduction control](references/stack.md#introduction-control).
Do not depend on `//skipcs` to end an active cutscene script.
Keep cutscene input, skip behavior, and missing actors separate from the feature under test.

**Never ask the user for credentials.** This is a local dev stack with
throwaway accounts; verification is expected to launch sessions on its own.
Default to one of the documented local accounts below (or provision a fresh
one) — do not stop to request a login. Use the user's real account only when the check requires their character.
Read those credentials from the environment.
Never request them in a prompt.

- **Local GM drive account** (default for GUI drives that need `!zone`/
  `!addtime`/`!setweather`): account `verilight`, password `TestPass!1234`,
  char `Verilamp` (gmlevel 5). This is the LSB provisioning-doc example
  credential for this machine's dev DB — safe to type into launch commands and
  logs, not a real secret. Launch:
  `cargo run -p kuluu --features native-window -- --agent-listen auto play verilight 'TestPass!1234' Verilamp`
- **Real character** (the user's) — credentials come from env
  (`FFXI_USER`/`FFXI_PASS`/`FFXI_CHAR`); never commit or log them. Use only
  when the check specifically needs the user's own character/progress.
- **Fresh provisioned chars** (`provision` + `create-char`, no DB teleport
  needed) — full E2E vehicle including zone changes and Mog House entry. Two corrected bugs caused ignored packets from fresh characters.
  The c2s datagram header must match the last subpacket sequence (`session.rs::datagram_header_id`).
  The introduction arrives in 0x00A LOGIN (`decode::ZoneInEvent`) and requires a 0x05B response.
  Without that response, the character remains InEvent and rejects 0x05E/0x0E7. If those symptoms ever return, check the
  sync/header invariant first (`map_networking.cpp:419-428`).
- **Ephemeral fixture chars** (`kuluu/tests/common/mod.rs::EphemeralChar`)
  — created against the live lobby + DB, gmlevel set pre-first-login; what the
  integration tests use. If accounts AUTO_INCREMENT exceeds the fixture sentinel accid scheme, the lobby rejects character selection.
  The log shows "mismatched character name" and the test fails at the 0x02 acknowledgement. Also rebuild `target/debug/kuluu-mcp` — the agent_session test
  spawns it without rebuilding.

## Reporting

Report the verdict and its observations.
Include event lines, trace lines, server logs, and screenshot paths.
Keep raw captures (`events.jsonl`, `client.log`) until report delivery. Anything driven around but not
observed (e.g. a GUI leg that needs human eyes) gets named explicitly rather
than silently skipped. Probes off the happy path (wrong zone, dead server,
double-send) are worth a line each even when they hold.

## Visual evidence in PRs

Use [kuluu-review](../kuluu-review/SKILL.md) for PR evidence criteria and claim boundaries. Open captures and inspect their contents before citing or publishing them; use the GUI reference's freshness checks and native-video fallback for black or stale frames. Keep raw artifacts locally and attach/embed reviewer-accessible media in the PR body. Record exact capture or publication blockers rather than treating a successful capture command as visual verification.

Upload inspected media directly with a current GitHub CLI:

```bash
gh pr edit PR_NUMBER --attach '/absolute/path/frame.png#Observed result' --attach /absolute/path/clip.mp4
```

Without a body flag, this appends to the existing body. With `--body-file`, references to the attached local paths are rewritten to uploaded URLs. Check `gh pr edit --help` for `--attach`; older CLI versions lack it. Read back the PR body after upload, including on failure because partial uploads can still update it. See the [official command documentation](https://cli.github.com/manual/gh_pr_edit).

## Recording evidence (feeds the stop-hook gate)

The stop hook `.agents/hooks/stop.d/25-verify.sh` requires fresh evidence for changed production Rust and shader source.
It includes committed edits and edits in other worktrees. Record evidence before the final report:

```
.agents/skills/verify/scripts/record-evidence.sh \
  --verdict pass --surface visual --summary "<what was OBSERVED>" \
  --build target/release/kuluu --inspection "<what the captured pixels show>" \
  --artifact artifacts/verify/zone.png
```

Open the image first: the inspection hook records its content hash. Video checks
include inspected frame captures as artifacts. Use `--surface session` with
captured event/log evidence for protocol work, or `--surface audio` with a
recording and `--inspection` describing what was heard. Repeat `--surface` for
changes spanning surfaces. Log files and passing unit tests cannot satisfy a
visual pass. Source, build, and artifact hashes bind the marker to the inspected
worktree; the build and capture must postdate the relevant edit.

For a concrete blocker, use `--verdict blocked --summary "<exact failure>"
--artifact <diagnostic.log>`. The final report must say verification is blocked
and include that exact summary; keep the bead open. `fail` never passes the
completion gate. A `waived` marker requires `--authorization "<user's explicit
opt-out>"` and disclosure of that quote in the final report. Do not self-waive
visual changes. Verify and record after the final source edit.

## Maintenance

Add durable verification findings to the applicable reference file with the fix.
Examples include environment failures and improved drive procedures. This skill rots
fastest at the env-gotcha layer.

## Subagent delegation

Verification runs (headless captures, GUI drives, retail comparisons) should run in subagents to keep the main context clean:

- Use a lower-cost available model for mechanical collection and a capable available model for judgment. If the harness does not offer an appropriate override, inherit the current model; do not request unavailable model names.
- Separate session readiness from visual evidence: use socket snapshots for zone-in, and inspect changing frames for rendering. See the native window-video fallback in `references/drive-gui.md` when screenshots are black or stale.
- Main agent receives only verdicts + evidence paths (artifacts/verify/...), not raw screenshot streams.
- Give subagents the exact scripts (`scripts/`, `references/drive-headless.md`, `references/drive-gui.md`) and the specific criteria to check; they should not improvise scope.

## Autonomy

Complete authorized verification without asking "what next?".
Collect evidence, record the result, and clean up owned temporary resources.
Preserve the final client state requested by the user.
Use normal logout when closing an owned game session.
Do not extend authorization to unrelated destructive or external actions.
