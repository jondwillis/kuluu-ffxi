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
