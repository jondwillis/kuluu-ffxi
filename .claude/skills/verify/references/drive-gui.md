# GUI drive: native window + MCP attach

For changes observable only in pixels/audio: rendering, HUD, camera, minimap,
materials, input-driven movement. The pattern is **window renders, agent
drives the session underneath, screenshots are the evidence**.

## Launch with the agent socket

```bash
cargo run -p ffxi-client --features native-window -- \
  --agent-listen auto play <user> '<password>' '<Char Name>'
```

Credentials are **positional args to `play`** — the GUI path reads no
`FFXI_USER`/`FFXI_PASS`/`FFXI_CHAR` env vars (only headless test fixtures
do); launching without them leaves the in-window launcher waiting for input
while the log looks alive (zone geometry loads behind the launcher).

`--agent-listen auto` (or `FFXI_AGENT_LISTEN=auto`) writes
`$TMPDIR/ffxi-agent.pid` with the unix-socket path — but that file can be
stale from a dead run; trust it only if its `pid` is alive, else glob
`$TMPDIR/ffxi-agent-<pid>.sock` for the live process. The GUI session runs
the full reactor, so goals work.

## Attach ffxi-mcp to the running window

```bash
FFXI_ATTACH=auto target/debug/ffxi-mcp
```

(`ffxi-attach` server in `ffxi-agent/.mcp.json` is the canonical config.)
Same tool vocabulary as standalone — `path_to`, `request_zone_change`,
`wait_for_event`, `scene://current` — but everything you do is rendered live
in the window. This is how you set up a visual scene programmatically: walk
the char to the right spot, trigger the zone change, spawn the state you need
to see.

## Capturing evidence

- **In-client**: the `/screenshot [path]` slash command (alias `/ss`) saves
  the primary window to PNG — but slash commands are typed in the window, so
  this needs the user's hands or a scripted keystroke you don't have.
- **External (agent-usable)**: capture the window from macOS:

  ```bash
  osascript -e 'tell app "System Events" to get id of first window of (first process whose name contains "ffxi")' 2>/dev/null
  screencapture -l <window_id> -x /tmp/verify-<what>.png    # -x = no sound
  ```

  Read the PNG back with the Read tool to actually look at it before citing
  it as evidence — a black or half-loaded frame proves nothing.
- Screen recordings for motion bugs (jank, camera): `screencapture -v` or ask
  the user to observe; say precisely what to look for.

## Keystrokes via System Events (menus ARE drivable)

The agent socket carries session-level `AgentCommand`s only, but macOS can
inject real keystrokes, which exercises the whole input layer (dialog
choices, menu stacks, the Items window — verified working for the Mog Menu
storage flow):

```bash
osascript -e 'tell application "System Events"
    set frontmost of (first process whose name contains "ffxi") to true
    delay 0.4
    key code 125  -- Down (126 Up, 123 Left, 124 Right)
    delay 0.3
    key code 36   -- Enter
end tell'         -- key code 53 = Escape
```

Needs Accessibility permission for the invoking terminal. Keep `delay`s
≥0.3s; screenshot after each step and Read it — keystrokes are fire-and-
forget.

## What still needs human eyes

- WASD/autorun movement *feel* (wall-slide, re-ground) — a scripted
  key-hold isn't a human hand
- Chase-camera orbit/zoom and camera collision feel

For these: set the scene up via attach (position, zone, targets), then hand
off with exact instructions — "walk into the north wall and watch whether the
camera clips into your head" beats "check the camera". `move` via socket
teleports the session position but bypasses the input-layer systems, so it
does NOT exercise input-driven bugs — don't let a socket `move` masquerade as
a movement test.

## Gotchas

- macOS: the Bevy/winit loop owns the OS main thread; the window opens on the
  user's desktop — tell them before spawning it.
- `screencapture` needs Screen Recording permission for the invoking terminal.
- One GUI client at a time: it holds the char's session; a parallel headless
  login with the same char will fight it (ghost-session lockout, stack.md).
- **Don't run `scripts/checks.sh test` while a GUI session is live** — the
  `agent_session` integration test logs into the same local LSB and kicks
  the running session mid-verify. Gate first, then launch.

## Session gotchas (2026-07-19)

- **Console lock kills capture**: if the macOS session locks (`CGSSessionScreenIsLocked=1` via `Quartz.CGSessionCopyCurrentDictionary()`), `screencapture` returns hard errors or solid black and System Events sees 0 windows — regardless of TCC grants. Check this FIRST when captures come back black; only a human unlock fixes it.
- **Agent-socket `chat` bypasses the client's local `/`-command parser** — it sends a raw wire SAY. Server-side `!` GM commands work through it; client-side `/` commands (e.g. `/lights`) need real keystrokes into the chat bar.
- **Agent-socket `move` persists server-side** (position survives relaunch); it is not a client-only hack. It also doesn't guarantee nearby NPCs stream in — prefer walking the last stretch for entity-visual checks.
- **GM drive char**: `Verilamp` (gmlevel 5) on the local throwaway `verilight` account; `!zone`/`!settime` work as socket chat. Fresh `create-char` chars have gmlevel 0 and the server silently ignores `!zone`. There is no `!settime`; use `!addtime <offset_in_seconds>` (LSB `scripts/commands/addtime.lua`, permission 5) — it resets-then-adds an earth-clock offset that indirectly drives Vana'diel time, so the same offset issued later in the session (after real time has elapsed) lands at a later Vana'diel time than the first call. `!setweather NONE` (permission 1) clears overcast/rain so directional shadows are actually visible — cloudy weather flattens lighting enough to hide cast shadows.
- **Known intermittent**: `slab_allocator Use-after-free` burst at zone-in can black out all zone geometry for the whole session (kuluu-172i); relaunch once before diagnosing rendering changes.
- **`nc -U` hangs the harness**: BSD `nc` (macOS) has no `-q`/idle-timeout that reliably closes after one command; `nc -U -w N <sock>` still blocks past N waiting on the socket, gets backgrounded by the harness, and — worse — the abandoned connection holds the agent socket's single-peer slot open so every subsequent send silently no-ops (`agent socket peer connected` never logs again) until you kill the stray `nc`. Use a one-shot Python `socket.socket(AF_UNIX, SOCK_STREAM)` with `settimeout()` + explicit `close()` instead of shelling out to `nc`.
- **`screencapture -l <window_id>` can return a stale cached frame for an occluded/background window** — HUD clock and other live state won't advance across captures even though the process is healthy and ticking. Bring the target frontmost first (`osascript … set frontmost of process "ffxi-client" to true`) and give it a beat before each capture, not just once at the start.
- **Agent-socket `move` can get clamped back onto the navmesh's nearest valid vertex** if the requested `(x,z)` at a fixed `x` isn't on a connected walkable surface (e.g. repeatedly increasing `z` while holding `x` constant can render the *same* spot every time — position telemetry echoes the requested coordinates, but the rendered transform snaps back). If teleporting through a gate/tunnel appears stuck, vary `x` as well as `z` rather than assuming the socket is broken.

## Focus-less driving: what works, what doesn't (2026-07-20)

Reading state over the socket is reliable; *driving player movement* over it is not — the GUI is a command **source**, so the socket can't reach the `input.rs` WASD movement path where re-ground/collision bugs actually live.

- **Read self position (reliable)**: one-shot Python AF_UNIX client, send `{"cmd":"snapshot"}\n` (AgentCommand is internally tagged, `"cmd"`, snake_case), read ~1s, keep the last `{"type":"position_changed","pos":{"pos":{x,y,z},...}}`. The stream is mostly `net_stats` spam — filter by `type`. (Same reason `nc` hangs, above: use a bounded Python read.)
- **`path_to` (reactor straight-line goal) did NOT move the local player** in a GUI session even with the reactor's navmesh loaded (it loads its own copy from `vendor/server/navmeshes/<Zone>.nav`, separate from the GUI's fetch-cache nav). `move`/`path_to` route through the reactor/session, **not** through `input.rs::dispatch_movement_system`, so they exercise a *different* grounding path than WASD and can't reproduce WASD-only bugs. Don't rely on them to reproduce movement/collision/re-ground issues.
- **WASD is only drivable via real keystrokes, and that's fragile**: `osascript … keystroke`/`key down` steals focus back to the invoking terminal, key-*hold* often doesn't register as continuous movement, and multi-word slash commands (`/debug heights`) get garbled/split. When focusing, there are **two `ffxi-client` processes** — pick the one that actually owns a window (`repeat with p in (processes whose name is "ffxi-client") … if (count of windows of p) > 0`), then `AXRaise` before each keystroke burst.
- **Gap → kuluu-0pof**: there is currently no socket/MCP path to (a) inject simulated movement input through `dispatch_movement_system` or (b) trigger `/debug heights` and read its server/nav/mzb numbers back. Until that lands, reproduce input-driven grounding bugs **offline** instead — load the real zone navmesh (`ffxi_nav_recast::fetch`) and MZB collision (`load_mzb_placed(zone_dat_id)`), walk a synthetic path carrying the height hint forward, and compare per-column (this pinned kuluu-nvqx deterministically without a live session).
