# GUI drive: native window + agent socket

For changes observable only in pixels/audio: rendering, HUD, camera, minimap,
materials, input-driven movement.

**Drive focus-free by default.** Verification runs on the human's own desktop
while they are using it. Everything except menu navigation can be driven and
captured without the window being frontmost, so stealing focus is a choice you
make deliberately for the few things that need it — not the default posture.
The one condition you cannot escape: macOS stops rendering a **fully occluded**
window, so *some* part of the client must stay on screen. Any sliver is enough;
focus is not.

## Hold the display awake FIRST

```bash
caffeinate -d -u -t 5400 &      # before launch, held for the whole session
```

Skipping this is the single most common way a GUI drive dies. When the host
display sleeps or detaches, winit logs `Monitor removed <id>` and the window
count drops to zero, which Bevy treats as a normal quit — so the client exits
through its full teardown path with **no panic and no error**, and the log tail
looks like a clean voluntary shutdown rather than a fault:

```
bevy_winit::system: Monitor removed 675v0
bevy_window::system: No windows are open, exiting
kuluu::view_native::exit_watchdog: teardown checkpoint stage="app.run() returned — winit loop exited cleanly"
```

Read that signature as *the display slept*, not as a client bug — it has cost
whole verification sessions to misdiagnose. `-u` (simulate user activity) is
needed alongside `-d`: once a monitor has actually been removed, `scripts/
capture.sh` returns blank frames until a real wake, so an already-asleep
display must be woken, not merely kept awake from that point on.

## Launch with the agent socket

```bash
.agents/skills/verify/scripts/launch.sh /tmp/verify-client.log
```

That is the whole launch. It uses the local GM drive account, passes
`--unfocused --mute`, waits for an in-zone snapshot from its own socket, and prints that socket path. Readiness must not depend on `sub_opcodes`: that debug log can be suppressed while a session is healthy. Socket existence alone proves IPC availability, not zone-in.
Export `FFXI_VERIFY_SOUND=1` to keep audio on when the change under test is
audio; `FFXI_VERIFY_USER`/`_PASS`/`_CHAR` override the character.

**Verify against `--release`.** launch.sh runs `target/release/kuluu` by default
and errors with the build line if it is missing:

```bash
cargo build -p kuluu --features native-window --release
```

The dev profile is Cranelift with no optimisation; measured on this repo it
zones in at **0.5-0.6 fps with 1.5-2.5s frame spikes**. Anything short-lived —
a cutscene fade, a cast bar, a hit flash, a weather transition — is then
unsamplable, and you will misread "I never caught it" as "it never happened".
Native viewer tests live in the `kuluu` library: use `cargo test -p kuluu
--lib --features native-window <filter>`. `--bin kuluu` can report zero tests.
For renderer tests with that feature set, select both `-p kuluu-render -p kuluu`;
`native-window` belongs to `kuluu`, not `kuluu-render`.

The longer one-off release build pays for itself in the first drive. Set
`FFXI_VERIFY_PROFILE=debug` only when the check genuinely needs
`debug_assertions` or a dev-only feature.

It also hands focus back to whatever app was frontmost. This matters: macOS
activates a newly launched app at the *process* level, which winit's
`focused: false` does not suppress — so a bare launch yanks the user out of
full-screen video even with `--unfocused`. Restoring afterwards is the only
lever available from outside the client, because Bevy builds the winit event
loop itself and exposes no macOS `ActivationPolicy` hook. `--unfocused` is
still worth passing: it keeps the window from being made key, so the blip is
shorter.

Doing it by hand instead — **do not ask the user for credentials** (see
SKILL.md "Character strategy"):

```bash
target/release/kuluu --agent-listen auto play --unfocused --mute \
  verilight 'TestPass!1234' Verilamp
```

`verilight`/`TestPass!1234`/`Verilamp` (gmlevel 5) is this machine's throwaway
dev account — a documented local example credential, not a secret; type it
into launch commands freely. Use the user's real env-var account only when the
check needs their own character.

Credentials are **positional args to `play`** — the GUI path reads no
`FFXI_USER`/`FFXI_PASS`/`FFXI_CHAR` env vars (only headless test fixtures
do); launching without them leaves the in-window launcher waiting for input
while the log looks alive (zone geometry loads behind the launcher).

`--agent-listen auto` writes `$TMPDIR/ffxi-agent.pid` with the unix-socket
path — but that file goes stale across the cargo-wrapper→binary re-exec and
after a dead run. Resolve the socket from the client log instead
(`grep -ao "/var/folders[^ ]*ffxi-agent-[0-9]*\.sock" <log>`) or glob
`$TMPDIR/ffxi-agent-*.sock` newest-first. The GUI session runs the full
reactor, so goals work.

Launch it with the harness's background mechanism (`run_in_background`), not a
detached `&` subshell — a subshell-detached client gets reparented and its log
stops growing mid-run.

## What is focus-free (almost everything)

| Need | Command | Notes |
|---|---|---|
| Session state, chat, GM `!cmds`, actions, zoning | agent socket `AgentCommand` | pure IPC, never touches the window |
| **Talking to an NPC / triggering its event** | `action` + `kind: talk` | see below — no keystrokes needed |
| **Answering an event's dialog choice** | `end_event_choice` / `end_event` | see below |
| Movement through the real `input.rs` path | `debug_drive` / MCP `walk` | kuluu-0pof; exercises heading, wall-slide, re-ground |
| Grounding numbers | `debug_heights` / MCP `debug_heights` | logged under `tracing target: debug_heights` |
| **Screen capture** | `scripts/capture.sh <out.png>` or MCP `screenshot` | kuluu-wwwv; GPU readback, see below |

### Driving NPC events (no keystrokes)

Talking to an NPC is a socket command, not a keypress — `Tab`+`Enter` through
System Events is never needed for this, and trying it is a dead end (the
synthesised `Enter` does not reach the client's interact binding, so no 0x01A
goes out and the log stays silent):

```jsonc
// talk to Camereine (the San d'Oria chocobo renter)
{"cmd":"action","target_id":17719343,"target_index":47,"kind":{"kind":"talk"}}
// answer her prompt: fields come from the event_dialog payload, NOT from the
// socket's own event_id -- see the field-name trap below
{"cmd":"end_event_choice","event_id":17719343,"act_index":47,"event_num":599,"choice":0}
{"cmd":"end_event"}
```

`ActionKind` is internally tagged, so `Talk` nests as `{"kind":"talk"}` inside
the `kind` field. Watch for `cutscene_started` / `event_start` / `event_dialog`
on the socket to confirm the event actually opened, and read `event_para` — it
tells you *which* event the server picked (e.g. a chocobo renter sends 602
"you lack the license" rather than 599 "rent" until the key item exists, and
the two look identical from the outside until you read that field).

**`EndEventChoice`'s field names lie, and getting them wrong fails silently.**
Two of the four do not mean what they are called
(`kuluu/src/view_native/text_input/mod.rs::confirm_dialog_choice`, which is the
only correct reference):

| Field | What to send | NOT |
|---|---|---|
| `event_id` | the dialog's **`npc_id`** | the socket's `event_id` |
| `act_index` | the dialog's `act_index` | |
| `event_num` | the dialog's **`event_para`** (e.g. 599) | `0`, or the zone |
| `choice` | 0-based index into the prompt's options | |

EVENT_END validates against the event id the trigger carried in `EventPara`.
Send the wrong pair and the server **ignores the choice with no error on either
side** — the client stays in the event, nothing advances, and it looks exactly
like the scene is broken. This cost a whole verification session: a chocobo
rental was reported as "no fade renders" when in truth the choice never landed,
and picking the same option by hand fades, zones, and mounts correctly. If a
choice appears to do nothing, re-check these two fields before you believe a
rendering bug.

Getting `target_id`: `snapshot` only replays entities that recently *upserted*,
so a static NPC standing still is usually absent from it. Ask the DB instead
and pick the id in the zone's range (neighbouring NPCs in the same zone share
the high bits):

```bash
docker exec server-database-1 mariadb -uxiadmin -ppassword xidb -N -B \
  -e "SELECT npcid, name FROM npc_list WHERE name='Camereine';"
```

`target_index` is `npcid & 0x7FF` **for NPCs** — verify it against a
neighbouring NPC's `act_index` in the snapshot before relying on it. (This does
*not* hold for player characters; see the `accounts_sessions.targid` gotcha
below.)

### GM `!cs <id>` cannot verify a cutscene

`!cs` starts the event on the **player** entity. The client then looks the id up
on the player's block, misses, and falls back to the zone master block — where a
different copy of that id may live. The observable result is an event that
"fires" (the log shows `event_dialog: resolved elsewhere source=ZoneMasterBlock`)
while nothing renders, which reads exactly like a broken renderer. Always
trigger through the real NPC with `action`/`talk`. Confirm the block you wanted
is the one that ran with the offline harness first:

```bash
cargo run -q -p ffxi-event --example zz-event-drive -- <zone> <event id> [params...]
# prints which blocks own the id, then every frame/choice/wait to the end
```

### Capture

```bash
.agents/skills/verify/scripts/capture.sh artifacts/verify/<what>.png
```

This sends `{"cmd":"screenshot","path":...}` over the socket, firing the same
`ScreenshotRequest` the `/screenshot` slash command does. Bevy captures by
reading the render target back off the GPU (`copy_texture_to_buffer` +
`map_async`), so unlike `screencapture -l <window_id>` it needs no Screen
Recording permission, never raises the window, and cannot hand back the stale
cached frame the window server keeps for a background window. Output is the raw
client frame at backing resolution — no macOS title bar to crop around.

The write is async, so the script waits for the file and checks for blank pixels.
A black readback is not proof of occlusion: it can also occur with an unlocked
console and a visible, rendering window. Pass the known socket when clients run
in parallel; the fallback derives its PID from that socket (or accepts an
explicit third argument for custom socket names). It raises only that process,
re-captures, and restores the prior process by PID,
logging `FOCUS WILL BLIP` so you know the human was interrupted. Correct
evidence beats zero disruption; a ~1s blip is cheaper than a black PNG being
cited as proof. If it remains blank after raising, the helper exits 2. Check console lock and
window state, then use the native video fallback below; the black artifact is
not citable.

Launching unfocused makes this fallback more likely, since nothing guarantees
the window ends up visible. Leaving the client somewhere it stays partly
on screen (a free corner, a second display) reduces the need to raise it.

Setting `frontmost` alone does not unminimize a window. The capture helper
clears `AXMinimized` and applies `AXRaise` on retry. For manual drives, do the
same before resending any input issued while the window was minimized.

Read every PNG back with the Read tool before citing it. A guard reporting
`lit=100%` only proves the GPU drew *something*.

**Never cite a black frame captured by the raw `{"cmd":"screenshot"}` socket
command.** That path skips capture.sh's blank check, and an occluded window
returns solid black — indistinguishable from a successful fade-to-black, which
is precisely the change you would be trying to verify. Confirm with capture.sh
(it raises and re-captures) before concluding a fade rendered. The safe pattern
for sampling a short-lived visual is: capture.sh once to prove the window is
live, then burst raw screenshot commands, then capture.sh again.

To sample faster than capture.sh's ~700ms round trip, send the burst down **one**
socket connection — the client services them back-to-back at roughly frame pace:

```python
cmds = [{"cmd": "action", ...}]
cmds += [{"cmd": "screenshot", "path": f"artifacts/verify/f{i:02d}.png"} for i in range(14)]
send(cmds, collect=9.0)   # ~200ms apart in practice
```

Then compare frames numerically rather than by eye — a fade is a luminance
change, and 13 near-identical means say "nothing happened" far more clearly than
13 screenshots do:

```python
from PIL import Image
im = Image.open(p).convert("L").resize((160, 100))
px = list(im.getdata()); print(p, sum(px) / len(px))
```

### Native window video when screenshots fail

In the 2026-09-09 Selbina movement verification, GPU screenshots were black
and `screencapture -l` stills were frozen while socket snapshots reported moving
actors. Native window-only video captured the actual motion. Do not repeatedly
retry stills or treat non-black pixels as proof of a current frame.

Resolve the window for the **known test PID**, not the first process named kuluu:

```bash
osascript -l JavaScript -e '
function run(argv) {
  ObjC.import("CoreGraphics");
  ObjC.bindFunction("CFMakeCollectable", ["id", ["void *"]]);
  const windows = ObjC.deepUnwrap($.CFMakeCollectable(
    $.CGWindowListCopyWindowInfo(0, $.kCGNullWindowID))) || [];
  return JSON.stringify(windows.filter(w =>
    w.kCGWindowOwnerPID === Number(argv[0]) && w.kCGWindowLayer === 0 &&
    w.kCGWindowBounds.Width > 200 && w.kCGWindowBounds.Height > 200));
}' "$client_pid"
```

Select that process's main game window from the returned IDs and bounds.
After checking `screencapture -h` for support, record a bounded window-only clip:

```bash
screencapture -v -V 20 -l "$window_id" artifacts/verify/movement.mov
```

If it needs foreground capture, warn the user, raise/unminimize only that
process, and restore the previous foreground PID afterward. Keep capture and
the movement driver in the same long-running invocation, with the recorder in
a subprocess and the driver on an independent monotonic schedule. Serial
screenshots can otherwise throttle a requested 5 Hz move stream. Save command
timestamps; inspect frame count, changing actor poses, and the relevant motion
intervals before calling the recording evidence. A protocol trace alone cannot
prove rendered motion. Stop visual retries if the bounded video is also stale.

For remote movement, vary both speed and packet spacing: include ordinary
running steps larger than any correction/snap threshold, sparse updates, stops,
and stairs. Record received position-change intervals as well as sent commands;
server coalescing can change the cadence. Inspect travel and gait throughout
each interval, not just endpoints: repeated run/walk/idle selection can restart
the animation even when position stays within confirmed bounds. Pair the video
with frame-level regressions that bound displacement and count gait changes.

For two-PC checks, use two separate local fixtures and retain both process IDs
and sockets. Disconnect only those sessions during teardown. A readiness
failure is not a failed login: disconnect through an already-created socket
before terminating the process, or the next login may hit the documented
`lpkt_next_login (view)` ghost-session timeout.

### Talking to the socket

Use a one-shot Python `AF_UNIX` client with `settimeout()` and an explicit
`close()`. Do **not** shell out to `nc -U`: BSD `nc` has no reliable
idle-timeout, blocks past `-w`, gets backgrounded by the harness, and the
abandoned connection holds the socket's single-peer slot open so every later
send silently no-ops until you kill the stray process.

`AgentCommand` fields are exact and a wrong key is dropped **silently** — the
socket accepts the line, nothing errors in the client or map log, and the
command never happens. `chat` is `{"cmd":"chat","kind":0,"text":"!hp 9999"}`;
sending `message` instead of `text` deserializes to nothing. Confirm a GM
command actually landed (re-`snapshot`, check the value moved) before
concluding the server rejected it. Variant names come from `AgentCommand` in
`kuluu/src/state.rs` — read the enum rather than guessing. `ActionKind`
is internally tagged, so a cast nests as
`{"kind":"cast_magic","spell_id":896,…}` inside the `kind` field.

## What still needs focus

**Menu navigation and anything typed.** The socket carries session-level
commands, not keystrokes, so the main menu, chat bar, and Tab-targeting need
real key events through System Events — which requires the process frontmost.

Reach for this only after checking that no `AgentCommand` covers what you want:
the socket vocabulary is wider than it looks, and several things that read like
"menu navigation" are commands (`action`/`talk` for NPC interaction,
`end_event_choice` for a dialog prompt, `treasure_lot`, `custom_menu_respond`,
`change_job`, `mog_house_exit`). Read the `AgentCommand` enum in
`kuluu-session/src/state.rs` before synthesising a keystroke — it is
`#[serde(tag = "cmd", rename_all = "snake_case")]`, so each variant's wire name
is its snake_case name.

```bash
osascript -e 'tell application "System Events"
    set frontmost of (first process whose unix id is <pid>) to true
    delay 0.6
    key code 48    -- Tab (36 Enter, 53 Esc, 125/126/123/124 arrows, 27 main menu)
end tell'
```

Resolve `<pid>` with `pgrep -f "^target/(release|debug)/kuluu"` — a bare
`pgrep -f kuluu` also matches the harness's own shell wrapper, and the
`osascript` then fails with "Invalid index". Needs Accessibility permission.
Keep delays ≥0.3s, capture after each step, and Read the result — keystrokes
are fire-and-forget.

Use `key code`, not `keystroke`, for keys the client binds physically:
`keystroke "/"` arrives as a text-insertion event, misses the client's
`KeyCode::Slash` chat-open binding, and falls through to in-world hotkeys
(observed: it opened the Job Abilities menu instead). `key code 44` (physical
Slash) works. Letters typed *into an already-open* text field are fine as
`keystroke`.

Because this steals focus, batch the keystroke legs of a run together instead
of interleaving them with focus-free work, and warn the user before you start
taking over their keyboard.

**Movement *feel*** (wall-slide, re-ground) and **chase-camera orbit/zoom feel**
still need human eyes. Set the scene up over the socket, then hand off with
exact instructions — "walk into the north wall and watch whether the camera
clips into your head" beats "check the camera". Socket `move` teleports the
session position and bypasses the input layer, so it does NOT exercise
input-driven bugs; use `debug_drive` for those.

## Gotchas

- macOS: the Bevy/winit loop owns the OS main thread; the window opens on the
  user's desktop — tell them before spawning it.
- Bevy's unfocused update mode is `reactive_low_power` at 60Hz, so a background
  window keeps rendering and stays capturable. A **hidden** app (`Hide`, or
  System Events `set visible to false`) does not, and un-hiding via
  `set visible to true` often doesn't stick — use `set frontmost … to true`.
- **Console lock kills everything visual**: if the macOS session locks
  (`CGSSessionScreenIsLocked=1` via `Quartz.CGSessionCopyCurrentDictionary()`),
  captures go black and System Events sees 0 windows regardless of TCC grants.
  Check this first when captures are blank; only a human unlock fixes it.
- Ghost sessions: prefer a clean socket `disconnect` over `kill`. The map server
  holds a killed char for 2–5 min and the next login times out. To clear one:
  `DELETE FROM accounts_sessions WHERE charid=<id>` (Verilamp is 17455719).
  That table's `targid` column is also the authoritative live targid —
  `charid & 0x7FF` is **not** it, and the server rejects actions built on it.
- Agent-socket `chat` bypasses the client's local `/`-command parser and sends a
  raw wire SAY. Server-side `!` GM commands work; client-side `/` commands need
  real keystrokes.
- Agent-socket `move` persists server-side and can be clamped back to the
  navmesh's nearest valid vertex — position telemetry echoes what you asked for
  while the rendered transform snaps back. Vary `x` as well as `z` if a teleport
  looks stuck.
- One session per character. A GUI observer and a headless mover need distinct
  characters. Keep each session's PID and socket explicit when another client is open.
- **Don't run `scripts/checks.sh test` while a GUI session is live** — the
  `agent_session` integration test logs into the same LSB and kicks the running
  session mid-verify. Gate first, then launch.
- Known intermittent: a `slab_allocator Use-after-free` burst at zone-in can
  black out all zone geometry for the whole session (kuluu-172i); relaunch once
  before diagnosing a rendering change.
