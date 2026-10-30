---
name: retail-observe
description: >
  Observe and drive the original FFXI client under Wine, native Windows, or a VM.
  Use for client setup, screenshots, menu text, and retail behavior evidence.
  For the Kuluu remake, use the verify skill instead.
---

# Observe the original client

Use this skill when the answer depends on what the original client shows or does.
Identify the client build, server, and host before making a behavior claim.
Keep game files, credentials, and captures outside Git.

## Select one host procedure

Paths beginning `scripts/` are relative to this skill directory.

| Host | Procedure |
|---|---|
| macOS Wine | [host-macos.md](references/host-macos.md): `python3 scripts/macos.py --profile NAME ...` |
| Linux Wine/Proton | [host-linux.md](references/host-linux.md): `scripts/observe.sh` |
| Native Windows | [host-windows.md](references/host-windows.md): `scripts/observe.ps1` |

The macOS Wine procedure uses one private JSON profile for setup, launch, capture, and input.
The shell `.conf` profiles belong to the other host procedures and optional VM control.
Use `observe.sh` on macOS only for an explicitly requested VM or host OCR/pointer operation.
Linux and native Windows execution remain unverified against a running FFXI client.
Read [client-and-server.md](references/client-and-server.md) only when selecting an install, loader, or server.

## Check existing evidence first

Search the observation records for the requested behavior:

```bash
rg -n -i 'TOPIC' references/
```

Read only the matching records and the selected host procedure.
Use [retail-grounding](../retail-grounding/SKILL.md) to resolve source conflicts or behavior questions.

## Select the control method

In Codex, prefer `cua_repl` computer use when it can select and control the running client.
Inspect a fresh screenshot before sending input.
Keep the selected host procedure for setup, diagnostics, and supported capture operations.
If computer use cannot select or control the client, save its diagnostic.
Then use a supported host input method only when the active tool rules permit it.

In other coding agents, use the selected host procedure for capture and input.
Use Jev or the current agent's vision for decisions as described below.
Neither Codex tools nor Jev credentials are mandatory across coding agents.

## Drive within the requested task

1. Run the selected procedure's `doctor` command.
2. Capture the game window.
3. Use Jev or native vision to classify the current capture.
4. Send the selected input, then capture and classify the result.
5. Stop when the requested evidence is sufficient.

Prefer logical keyboard names from `scripts/lib/keys.tsv`.
For pointer input, divide capture pixels by the reported scale to obtain window coordinates.
Recalculate coordinates after a window move or resize.
Allow only one driver to control the client.
Stop if two consecutive inputs produce no visible change.

## Delegate routine decisions

Prefer a bounded Jev controller for routine login and dialogue when Jev is configured.
Give the controller an observable goal, permitted actions, time limit, and failure conditions.
Let it classify fresh captures and execute validated actions without full-model review of each image.
Stop for unexpected screens or two consecutive actions without the expected change.
For low confidence, permit at most two fresh-capture retries without input.
Keep the overall time limit.
Use the main agent for failed actions, unusual states, and key evidence images.
Do not duplicate every image inspection across agents.

On macOS, `decide` uses Jev when configured.
The result separates perception failures from caller policy decisions.
For `low_confidence`, capture a fresh frame or explicitly select native vision.
For `api_error` or `credential_error`, correct the reported access problem.
For `schema_error` or `capture_changed`, correct the response or capture binding before input.
Do not automatically send routine frames to a full model after a Jev failure.
Use native vision when explicitly selected or when a true exception needs inspection.
Validate its JSON with `decide --result` and the same task before use.
A bounded controller can execute decisions after schema, confidence, capture-hash, and caller-policy validation.
Pass the active caller policy with `--agreement-policy`.
The portable default, `caller`, requests a policy decision without changing the Jev classification.
Use `permitted` only when the active harness and session authorize the proposed agreement action.
If Codex requires confirmation for that action, select `confirmation_required`.
That result reports `policy_required`, not a vision failure.
Jev proposes actions; the caller applies authorization and the active harness policy.
For a user's own local-server launcher, macOS setup can store the explicit `--startup-enter` preference.
This performs one known startup Enter without model classification.
Read the [macOS host reference](references/host-macos.md) before configuring it.
If the active Codex policy requires confirmation, launch with `--agreement-policy confirmation_required` to suppress that input.
The stored user-run preference does not override an active harness policy.

Keep one input driver per client.
Inspect key evidence images before making a visual claim.

## Approval and evidence

Reuse authorization already given in the session.
A request to drive or verify a client covers routine navigation, dialogue, and reversible local test actions.
Continue those actions without repeated confirmation.
Reuse approval for the same screenshot destination and purpose.
Ask only when required authorization is missing or the requested operation changes scope.
This skill adds no confirmation step for an agreement screen.
Use the session authorization and the active coding agent's interaction policy.
If that policy requires confirmation or direct user action, identify the policy as the source of the requirement.
Do not present that requirement as a rule from this skill.
Do not infer authorization for unrelated agreements, system permissions, purchases, or item consumption.
Keep account credentials out of reports and saved profiles.
Honor the user's requested final client state; do not log out automatically.

Save captures under `artifacts/retail/` and inspect them before citing a visible result.
Report the tested build, action, capture path, observed result, and remaining limits.
Record durable game findings as dated observation records under `references/`.
Follow the root `AGENTS.md` rules for evidence, source provenance, and separate binary reading and implementation sessions.
Update host procedures for setup changes and `keys.tsv` for new logical keys.
