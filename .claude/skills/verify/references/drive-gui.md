# GUI drive: native window + MCP attach

For changes observable only in pixels/audio: rendering, HUD, camera, minimap,
materials, input-driven movement. The pattern is **window renders, agent
drives the session underneath, screenshots are the evidence**.

## Launch with the agent socket

```bash
FFXI_USER=... FFXI_PASS=... FFXI_CHAR=... FFXI_SERVER=127.0.0.1 \
  cargo run -p ffxi-client -- --agent-listen auto play
```

`--agent-listen auto` (or `FFXI_AGENT_LISTEN=auto`) writes
`$TMPDIR/ffxi-agent.pid` with the unix-socket path. The GUI session runs the
full reactor, so goals work.

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

## What still needs human eyes

The agent socket carries session-level `AgentCommand`s only — **no
keystrokes**. Anything driven by the input layer is out of reach:

- WASD/autorun movement (and everything downstream: wall-slide, re-ground,
  `dispatch_movement_system`)
- Chase-camera orbit/zoom and camera collision feel
- Menu/HUD keyboard navigation, slash-command entry

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
