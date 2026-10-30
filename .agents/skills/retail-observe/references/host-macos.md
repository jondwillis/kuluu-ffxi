# macOS Wine

Use `scripts/macos.py` from this skill directory for the Wine procedure.
It requires Python 3.9 or later and standard macOS tools; it requires no pip packages or jq.
Jev is optional. Native vision can inspect the same captures.
The keyboard helper uses AutoHotkey inside the game's Wine prefix.
Host CGEvent input did not control the tested FFXI client.

## Setup

Supply a working Wine build, its prepared prefix, your game installation, and a compatible xiloader executable.
On Apple Silicon, install Rosetta if the Wine build requires it.
Grant Screen Recording permission to the calling application.
The Wine keyboard path does not require macOS Accessibility permission.
Setup does not install Wine, game files, or client registry entries.

```bash
python3 scripts/macos.py --profile local setup \
  --runner /absolute/path/to/wine \
  --prefix /absolute/path/to/prefix \
  --install '/absolute/path/to/FINAL FANTASY XI' \
  --loader /absolute/path/to/xiloader.exe \
  --server 127.0.0.1 --loader-arg=--hairpin
```

Setup saves `~/.config/ffxi-observe/local.macos.json` with permissions `0600`.
It caches the loader and AutoHotkey under `~/.cache/ffxi-observe/`, outside the checkout.
It honors `XDG_CONFIG_HOME` and `XDG_CACHE_HOME`.
It verifies AutoHotkey v2.0.30 against the [official release checksum](https://github.com/AutoHotkey/AutoHotkey/releases/tag/v2.0.30).
For offline setup, add `--ahk-archive /path/to/AutoHotkey_2.0.30.zip`.
Run setup on each machine; absolute paths are local configuration.
If a recorded executable changes, verify the update before running setup with `--replace`.
The Wine executable hash does not cover every library in its distribution.

Keep credentials outside the profile.
Set `FFXI_USER` and `FFXI_PASS` in the environment before `launch`.
The command does not print loader arguments, but credentials can appear in local process arguments.

## Capture and input

```bash
python3 scripts/macos.py --profile local doctor
python3 scripts/macos.py --profile local launch
python3 scripts/macos.py --profile local capture /tmp/ffxi-before.png
python3 scripts/macos.py --profile local key menu 0.2
python3 scripts/macos.py --profile local capture /tmp/ffxi-menu.png
python3 scripts/macos.py --profile local key esc 0.2
```

Use `launch` only when the client is not running.

For a user-run local private-server profile, add `--startup-enter` to `setup` once.
This explicit preference stores one known startup Enter action.
Future `launch` commands use that preference without Jev or model classification.
The destination must be loopback.
Validated loader options include `--hairpin`, `--hide`, `--lang`, `--authport`, `--dataport`, `--viewport`, and `--profileport`.
Port values must be decimal integers from 1 to 65535.
Language values must be `JP`, `US`, `EU`, `0`, `1`, or `2`.
Use separate option and value arguments.
Server overrides, JSON configuration overrides, duplicate options, and unknown options are rejected.
These rules follow the [xiloader argument parser](https://github.com/LandSandBoat/xiloader/blob/main/src/main.cpp).
They validate the declared arguments; they do not independently verify a custom loader binary's behavior.
The launcher rejects an existing game window before it starts another client.
It waits up to 30 seconds for one new game window.
After that window appears, it waits five seconds and checks the same window again.
It then sends Enter once through the guarded Wine helper.
This option assumes that the selected local profile presents the known startup prompt.
It does not classify other dialogs or repeat input.
Configuration and launch output show the requested action and caller policy.
Existing profiles keep their previous behavior until this option is configured.

An active caller can override the stored user-run policy:

```bash
python3 scripts/macos.py --profile local launch --agreement-policy confirmation_required
```

If the active Codex policy requires confirmation for the startup action, use this override.
It starts the client but sends no startup input.
Use `--agreement-policy caller` when the active caller must decide authorization separately.
The user's own authorized launcher can use its stored `permitted` policy.
Do not change an active harness policy to enable the startup action.

Inspect captures to verify each visible result.
A successful key command proves submission, not the game's response.
Capture and input require one matching FFXI window.
The capture result includes the pixel dimensions, window dimensions, and scale.
Capture can read a unique offscreen host window and reports its on-screen status.
It wakes the display before host-window discovery.
This read-only option does not prove that hidden-window input works.
Input rejects a locked screen, ambiguous windows, unknown keys, and holds longer than five seconds.
A prefix lock excludes concurrent packaged inputs; other tools and human input can still interfere.
Background operation without macOS focus changes remains unverified.

If the host window is offscreen, use explicit offscreen control with a fresh packaged capture.
The capture must be unchanged and no more than 60 seconds old.
The helper checks the host window ID and process ID against that capture.
The unique guest-window, active-window, unlocked-desktop, and prefix-lock checks still apply.
The default input mode requires an on-screen host window.

```bash
python3 scripts/macos.py --profile local capture /tmp/ffxi-current.png
python3 scripts/macos.py --profile local --offscreen --frame /tmp/ffxi-current.png key enter 0.2
```

For `drive-dialogue`, add `--offscreen` before the command.
The controller binds each input to its own fresh capture and validated decision.

For literal text, use a private UTF-8 file or standard input.
The file must belong to the current user and have no group or other permissions.
Text permits one printable line of up to 4096 UTF-8 bytes.
The operation does not press Enter or report the text.
Inspect the result before you submit a chat command.

```bash
python3 scripts/macos.py --profile local text /private/path/input.txt
python3 scripts/macos.py --profile local text - < /private/path/input.txt
python3 scripts/macos.py --profile local wheel up 1
python3 scripts/macos.py --profile local wheel down 1
python3 scripts/macos.py --profile local drag 120 0 1
```

Wheel input permits `up` or `down` and a count from 1 to 20.
Start with one wheel step.
Text and wheel input use the same prefix lock and visible-window checks as keyboard input.
Literal text uses AutoHotkey `SendText`; braces and modifier characters cannot become key instructions.
Wheel input uses AutoHotkey `SendEvent` at the center of the selected game window.
It checks the active window and pointer target immediately before input.
It restores the pointer position after input and rejects a held mouse button.
AutoHotkey sends wheel events at the [current pointer position](https://www.autohotkey.com/docs/v2/lib/Send.htm#mouse-buttons).

For a right-button drag, specify horizontal displacement, vertical displacement, and hold duration.
Displacements use integer client pixels, with a maximum absolute value of 512 on each axis.
The duration must be at least one millisecond and no more than five seconds.
The drag starts at the client center and rejects an endpoint outside the client area.
It moves to the endpoint, holds the right button for the specified duration, then releases it.
It releases the button and restores the pointer if the operation fails.
Inspect a capture before and after each drag.

## Jev or native vision

For Jev, store the key in `~/.config/typesafe/api-key` with permissions `0600` and a private parent directory.
Use `--jev-key-file /private/path` during setup to select another key file.
Capture and input do not require this key.

```bash
python3 scripts/macos.py --profile local decide /tmp/ffxi-menu.png --character 'Exact Name'
```

Only `decide` sends an image to Jev.
It uses the pinned model `jev-1.14.0` and resizes a temporary image to the API limits.
Jev image support remains [experimental](https://docs.typesafe.ai/images).
The request includes `TypeSafe-Danger-Zone-Images: true` and an image asset referenced by `state.screen_image`.
Results distinguish `credential_error`, `api_error`, `schema_error`, `capture_changed`, `local_error`, and `low_confidence`.
Each failure supplies a diagnostic and capture hash.
Low confidence also supplies the classification, proposed action, and measured confidence.
Correct access or schema problems before retrying Jev.
For low confidence, capture a fresh frame or select native vision explicitly.
A Jev failure does not automatically transfer routine frames to a full model.
Use `--vision` to request native vision without an API call.
Use native vision for an explicit request or a true exception that requires inspection.
Save the returned JSON and validate it:

```bash
python3 scripts/macos.py --profile local decide /tmp/ffxi-menu.png \
  --character 'Exact Name' --result /tmp/vision-result.json
```

The default decision task covers login: `enter`, `wait`, or `stop`.
Use `--task dialogue` for plain story or NPC dialogue.
For plain dialogue, Enter requires a visible advance marker without choices or a menu.
It stops for choices, errors, or gameplay.
For low confidence, the controller waits and captures a fresh frame without input.
It permits at most two consecutive retries within the overall time limit.
A valid decision resets that retry count.
Agreement actions use the separate caller policy.
The dialogue task derives routine actions from the screen classification.
The minimum confidence is 0.9.
For an agreement, Jev separately proposes the visible action.

```bash
python3 scripts/macos.py --profile local decide /tmp/ffxi-dialogue.png \
  --character 'Exact Name' --task dialogue
```

The `decide` command sends no input.
The bounded controller applies the caller policy before input.
Its portable default, `--agreement-policy caller`, reports `caller_policy_required` for an agreement proposal.
When the active harness and session permit the action, select `--agreement-policy permitted`.
The controller can then execute that validated proposal within its action and time limits.
When Codex requires confirmation for that action, select `--agreement-policy confirmation_required`.
The result reports `policy_required` and preserves the Jev proposal.
Jev does not decide authorization.
Do not report a caller-policy requirement as a vision failure.

For routine dialogue, use the bounded Jev controller after the session permits screenshot uploads and game input.
Reuse approval for the same service and comparison scope.

```bash
python3 scripts/macos.py --profile local drive-dialogue /private/path/new-capture-folder \
  --character 'Exact Name' --max-actions 40 --max-seconds 300
```

The controller captures each state and executes only validated plain-dialogue decisions.
It allows a short interval after Enter before the next capture.
It waits while plain dialogue text is still appearing without an advance marker.
Local text recognition detects two unchanged dialogue results and stops further input.
Choices, low confidence, unexpected screens, and action or time limits also stop the controller.
It saves captures, hashes, decisions, and a summary in the new folder.
Inspect key evidence and exceptions with the current agent's vision.
Routine dialogue captures do not require repeated full-model inspection.

## Optional local OrbStack server supplement

`scripts/lsb-retail.compose.yaml` adds connect and profile services to an existing compatible LSB stack.
The database, world, map, search, account, and reachable zone addresses must already exist.
The supplement pins the server image used for the local login; check compatibility with the existing database schema.
Do not start it if another service occupies its ports.

```bash
export LSB_NETWORK=your-lsb-project_default
export LSB_SQL_PASSWORD=your-local-database-password
# Optional: LSB_SQL_HOST, LSB_SQL_DATABASE, LSB_SQL_LOGIN, LSB_WORLD_HOST
# LSB_VER_LOCK defaults to 2; use 0 only for an intentionally older local test client.
docker --context orbstack compose -p retail-observe \
  -f scripts/lsb-retail.compose.yaml config --quiet
docker --context orbstack compose -p retail-observe \
  -f scripts/lsb-retail.compose.yaml up -d
```

Add these arguments to setup for this supplement:

```bash
--loader-arg=--authport --loader-arg=57432 \
--loader-arg=--viewport --loader-arg=57003 \
--loader-arg=--dataport --loader-arg=57433
```

The distinct internal auth port avoids the default connect server's routing identifier.
Profile ports are `51220` and `51240`. Published ports bind only to `127.0.0.1`.

## Setup diagnostics and verification limits

Keep a Wine build and its prefix together; switching builds can stall prefix initialization.
A prepared prefix needs the client's PlayOnline install/interface registry values and FFXI settings.
Register `polcore.dll`, `FFXi.dll`, `FFXiMain.dll`, and `FFXiVersions.dll` for the selected installation.
The interface values must exist in the 32-bit registry view and contain the installation's real interface identifiers.
Use the installation's registration procedure rather than guessed values.
See [client-and-server.md](client-and-server.md) for loader and client-version compatibility.

`doctor` checks local dependencies and permissions; it does not prove login or correct client registration.
If no window exists, it reports a null window; launch before the visible input check.
If capture fails, check Screen Recording permission and the display state.
If the process runs but its host window is absent, try the packaged restore operation before another launch.

```bash
python3 scripts/macos.py --profile local restore
```

Restore requires an unlocked desktop and exactly one matching guest window.
It uses the prefix lock and sends no keyboard or mouse input.
It hides and shows that guest window once, while it preserves the window rectangle.
Guest restoration may not expose a hidden macOS application or a window in another Space.
Capture the restored window before continuing.
If inputs have no visible effect, stop before diagnosing a stale window or another active driver.
For explicit VM or host OCR/pointer work, use `observe.sh help` and `observe.sh doctor`.
That separate host input path requires Accessibility permission and uses shell `.conf` profiles.

On 2026-10-19, packaged Wine input opened the HorizonXI 2023 client's menu on Apple Silicon against local OrbStack LSB.
The captures were inspected. Jev and a separate vision subagent identified gameplay.
A clean installation on another Mac and background input remain unverified.
