# Headless drive: route first, then run it

Drive without putting anything on the user's desktop. The table below is both the
router and the contents: pick the row from **what evidence the request needs**, never
from what's easiest to launch. Each row states its window mode; that choice is not
reversible later — a session-only run cannot be asked to produce pixels after the fact.

| Request asks | Evidence that settles it | Window | Run it with |
|---|---|---|---|
| What does the server send? wire decode, zoning, chat, entity/spawn flow, reactor goals | JSON event stream + MCP resources + map-server log | **none** | §1 MCP (preferred) or §2 raw stdio |
| Did my change panic / hang / spam errors — looks don't matter | tracing log on stderr + exit code (+ one snapshot as liveness proof) | **offscreen** if the change touches `kuluu-render`/`view_native`; **none** if session-side | §Render-health runs. **This row never closes a rendering, camera, HUD or animation change** — those are only complete with an inspected capture of the affected view (AGENTS.md: rendered evidence is part of finishing UI work), so treat it as row 3 with the pixels step still open |
| Is it really drawn — prove the render, HUD, camera or animation | image file opened with `view_image`/`Read` | **offscreen**, or **visible** on hosts that can't present offscreen | §Taking screenshots (A pre-server box → B live key drive → C scripted fallback) |
| Which layer is broken, before hand-driving anything | test verdicts | none | §3 live integration tests |
| Movement feel, camera feel, timing in the player's hands | a human's eyes | **visible** | `references/drive-gui.md`, and say out loud what you're handing over |

Two rules that decide most mis-runs:

- **`play --headless` runs no render pipeline.** The flag drops to the stdio agent
  session (`kuluu/src/main.rs::Command::Play`) — no Bevy app, no wgpu device, no MZB
  geometry, no particles. A clean headless run proves nothing about code under
  `kuluu-render` or `view_native`; a render-side panic needs the pipeline actually up,
  which means an offscreen window (§Render-health runs). Session-only logs carry zero
  `kuluu_render` lines — that absence is expected, not a broken build.
- **Knob names come from the build, never from this file.** `kuluu animtest --knobs` (and
  `{"query":"knobs"}` over `FFXI_KEY_DRIVE`) print what cases/environment variables the running
  binary actually accepts; a name copied from documentation is a guess, and a guessed `ANIMTEST_*`
  silently does nothing. §Taking screenshots has both invocations.
- **Window modes are three, and only two work.** *Offscreen* = created hidden, then
  parked past every monitor and **shown**, so the swapchain keeps producing frames;
  nothing reaches the desktop (see §Offscreen window vocabulary). *Buried* = never
  shown: presents zero frames, every capture comes back all-black or lands no file at
  all. *Visible* = on the user's display — allowed only per the escalation ladder, and
  warned first.

Credentials follow `SKILL.md` §Character strategy: **never ask the user**. The local GM
drive account is documented there; a real character comes from `FFXI_USER`/`FFXI_PASS`/
`FFXI_CHAR` in the environment, never a prompt. Stack bring-up and env failure modes are
in `references/stack.md`; lifecycle is `scripts/lsb-stack.sh up|status|down`.

No command below passes a port: the client defaults are the scraped LSB values
(`ffxi_proto::login::LOGIN_AUTH_PORT` / `LOGIN_DATA_PORT` / `LOGIN_VIEW_PORT`, pinned by
test) and they match the published ports in `references/stack.md`. If `docker port
server-connect-1` disagrees with that table, pass `--auth-port/--data-port/--view-port` on
every launch of that session — a half-corrected relaunch reads as a server bug.

---

## 1. kuluu-mcp standalone (preferred)

Spawns the full supervisor→reactor→session pipeline and exposes MCP
tools/resources over stdio. This is the only headless path with the reactor
(goals, keepalive, event auto-dismiss) and the only one with event-driven
waits instead of log polling.

```bash
cargo build -p kuluu-mcp          # binary at target/debug/kuluu-mcp(.exe on Windows)
FFXI_USER=... FFXI_PASS=... FFXI_CHAR=... FFXI_SERVER=127.0.0.1 target/debug/kuluu-mcp
```

Drive it as an MCP server (`claude mcp add` / `.mcp.json` — see
`ffxi-agent/.mcp.json` for the canonical config). The high-value calls for
verification:

- `wait_for_event {kinds, timeout_ms}` — block until `zone_changed` /
  `entity_upserted` / `connected` / … fires. Use instead of polling.
- `read_resource scene://current` — entities, zone, self state as JSON.
- `read_resource diagnostics://session` — seq/sync counters, net health.
- `request_zone_change {line_id}` — zoneline/MH-door trigger (char must be
  standing in the rect; move there first with `path_to`).
- `snapshot`, `chat`, `cast`, `engage`, `follow`, `disconnect` — see
  `ffxi-agent/instructions/playbook.md` for the full vocabulary.

`FFXI_ATTACH=auto` mode attaches to an already-running client instead of
spawning its own — that's the GUI path, see `drive-gui.md`.

Reading what the server put around you: every nearby object is an
`entity_upserted` event and self is the one with `kind:"pc"` (its entity id
varies per login). Compute distance from self across upserts to see exactly who
the server placed in the area. Zone fixtures (lamps, torches) are **not**
entities — they come from the MZB, so they only appear once a render surface is
running (§Taking screenshots, A/B).

## 2. Raw stdio (`play --headless`)

Zero extra deps; JSON commands on stdin, typed JSON events on stdout, tracing
on stderr. This path uses the reactor's explicit agent profile, so goal commands
and fishing automation behave like MCP; it has no supervisor/reconnect layer or
event-driven MCP waits.

**Credentials flip between §1 and §2, deliberately.** §1 `kuluu-mcp` reads them from the
environment (`FFXI_USER`/`FFXI_PASS`/`FFXI_CHAR`); §2 takes them as **positional args** —
its env vars are ignored and it blocks on a `Username:` prompt instead. Copying one form
into the other is the usual cause of a run that appears hung.

**Setup and launch are separate commands.** In Git Bash `A && B & C` backgrounds the
whole chain, so kuluu's `< $D/in` redirect can race fifo creation and die with "No such
file or directory" before it ever connects (verified failure mode). Same discipline is
cheaper on Unix than clever there.

```bash
# Windows (Git Bash) — three commands, never chained:
D=/c/tmp/agent; rm -rf $D; mkdir -p $D; mkfifo $D/in

(exec 3>$D/in; sleep 900) &   # hold the fifo write end open for the session's lifetime
RUST_LOG=info ./target/release/kuluu.exe --server 127.0.0.1 \
  play verilight 'TestPass!1234' Verilamp --headless --mute < $D/in > $D/events.jsonl 2> $D/client.log &

sleep 12; head -c 3000 $D/events.jsonl    # connected -> zone_changed -> entity_upserted
```

Both blocks run the **release** binary (`cargo build -p kuluu --features native-window --release`,
§Render-health runs) so a drive exercises what ships; drop to `cargo run` only when the change
under test needs dev-profile assertions.

```bash
# Unix — no mkfifo needed at all:
D=$(mktemp -d)
(sleep 1800 | RUST_LOG=info ./target/release/kuluu --server 127.0.0.1 play \
  verilight 'TestPass!1234' Verilamp --headless --mute) > $D/events.jsonl 2> $D/client.log &
```

The pipe's write end stays open for `sleep`'s lifetime, and EOF ends the session cleanly.

- Commands are `AgentCommand` serde: `{"cmd":"snake_case", ...}` (state.rs).
  Events are `{"type":"snake_case", ...}`.
- Credentials are **positional args**, not env — env vars only feed the
  interactive launcher, which will otherwise block on a `Username:` prompt.
- Coordinate space in commands/events: `x` = native x, `y` = ground (native z),
  `z` = vertical (native y).
- Zoneline ids are the fourcc as LE u32 — look them up in
  `vendor/server/data/zones/<zone>/zone.yaml` zonelines (the fourcc key names each line).
- **The agent socket is Unix-only.** `--agent-listen`/`FFXI_AGENT_LISTEN` sits behind
  `#[cfg(unix)]` (`kuluu/src/main.rs`, `view_native/mod.rs`), so on Windows there is no
  socket to inject into and no `.sock` file to find — stdin held open by the pipe *is*
  your control channel. To drive a running session from another shell on Unix, launch with
  `--agent-listen auto` (or `FFXI_AGENT_LISTEN=<path>`) and send one JSON line per connection:
  `echo '{"cmd":"move",...}' | nc -U <socket-path>`.
- **Windows input-only injection** is a different channel: `FFXI_KEY_DRIVE=<port>` on a
  windowed run takes keystrokes, not `AgentCommand`s (§B). Keystroke injection never proves
  an agent command ran.

## 3. Live integration tests (canonical layer proofs)

In `kuluu-session/tests/`. They drive the real client against the real server and
**self-skip when the auth port or xidb is unreachable** — runtime verification
harnesses, not CI re-runs. Use them to bisect which layer is broken before
hand-driving. Note the package: `-p kuluu-session`, not `-p kuluu`.

```bash
cargo test -p kuluu-session --test play_lifecycle -- --nocapture   # auth→lobby→map→InZone→disconnect (~3s)
cargo test -p kuluu-session --tests                   -- --nocapture   # every session-layer test, self-skips included
```

| Test | Proves | Needs |
|---|---|---|
| `play_lifecycle` | auth→lobby→map→InZone→disconnect, stage order asserted | stack + xidb |
| `zone_change` | zone change reconnects with a rotated blowfish key | stack + xidb |
| `agent_session` | full MCP-driven session (transport floor); spawns `target/debug/kuluu-mcp`, so rebuild it first | stack + xidb |
| `disconnect_recovery` | map-server restart mid-session; **destructive, opt-in** via `RESTART_MAP_SERVER=1` | docker |
| `event_503_live` / `event_531_live` / `event_568_live` | scripted-event playback: cues in authored order, input-gated frames, `onEventFinish` outcome | stack + a registered install |
| `bastok_intro_live` / `chocobo_rental_live` | in-zone flows driven end to end, artifacts written per run | stack + install |
| `auction_search_live` | AH search-server browse + sale history over TCP `SEARCH_PORT`; no map session | stack |
| `delivery_box_live` | server-side delivery-box flow over the agent channel | stack + xidb |
| `action_dispatch` | offline: cast/weaponskill/job-ability subpacket layouts match the phoenix structs | nothing |
| `install_conformance` | what `scripts/checks.sh install` runs once per registered client install | an install |

They use the `EphemeralChar` fixture (`kuluu-session/tests/common/mod.rs`): isolated
account + char stamped into MariaDB, gmlevel set before first login. If a
manual flow fails where the matching test passes, diff your flow against the
fixture's — that delta is the bug or the blocker. Gotcha: when the accounts AUTO_INCREMENT outruns the fixture's sentinel accid
scheme, the lobby rejects the char select ("mismatched character name" in connect logs)
and the test dies at the 0x02 ack step.

## Offscreen window vocabulary

One env var, and its name misleads: `KULUU_WINDOW_HIDDEN=1`
(`kuluu/src/view_native/mod.rs::WINDOW_HIDDEN_ENV`) creates the window hidden and then
**parks it past every monitor and shows it**, so nothing reaches the desktop while the
swapchain keeps producing frames. Read "hidden" as *offscreen-presented*.

- **Never substitute a buried window for an offscreen one.** A window that is merely
  invisible presents no frames: `PrintWindow` returns all black and in-app readback lands
  no file. The parking step is load-bearing — if every capture is black, confirm the park
  still runs before adding waits, retries or capture-flag tweaks.
- **`--unfocused` is a different axis.** It launches without stealing focus (and macOS
  activates apps at process level regardless — `.agents/skills/verify/scripts/launch.sh` restores the frontmost
  app afterwards). A focused-out window must still stay un-occluded to keep rendering on
  macOS.
- **Escalation ladder for hosts that cannot present offscreen** (macOS occlusion stops
  drawing invisible windows; X11/Wayland without a compositor): warn the user, run one
  visible window on their display, drive focus-free over the agent socket, capture, exit,
  and restore focus. Session-only `--headless` runs are never pixel evidence.

## Render-health runs (panic/hang, pixels irrelevant)

Use when the question is "does my change survive running", and the change touches code
that only executes inside the render app. The window must exist; a capture is optional.

```bash
# Windows — build once, run parked:
cargo build -p kuluu --release --features native-window
mkdir -p /c/tmp/health-$(date +%Y%m%d)
KULUU_WINDOW_HIDDEN=1 RUST_LOG=warn \
  ./target/release/kuluu.exe --server 127.0.0.1 play verilight 'TestPass!1234' Verilamp \
  --mute > /dev/null 2> /c/tmp/health-<date>/client.log &

# Unix — identical invocation, binary and kill differ:
KULUU_WINDOW_HIDDEN=1 RUST_LOG=warn \
  ./target/release/kuluu --server 127.0.0.1 play verilight 'TestPass!1234' Verilamp \
  --mute > /dev/null 2> $D/client.log &
```

- **Run the release build.** The dev profile is Cranelift + no optimisation and renders
  zone-in well under 1 fps with multi-second frame spikes, which makes anything short-lived
  (a fade, a cast bar, a hit flash) unsamplable (`.agents/skills/verify/scripts/launch.sh` header). Override with
  `FFXI_VERIFY_PROFILE=debug` only when the change needs `debug_assertions`.
- Read the log for panics/`ERROR:` and the process for a non-zero exit; prove liveness
  separately (an agent-socket snapshot, or one typed command that answers) — silence is
  not health. Escalate to §Taking screenshots as soon as a symptom is visual; a panic-free
  log does not clear a rendering change.
- Prefer `.agents/skills/verify/scripts/launch.sh <logfile> [play args...]` when it fits: release binary by
  default, `--unfocused --mute`, local drive-account defaults, and it prints the resolved
  agent socket path.

## Taking screenshots without a desktop

Three paths, in that order; each names the symptom that means *you picked the wrong one*.
Everything lands in a dated folder (`/c/tmp/<what-you-check>-<YYYYMMDD>/` on Windows,
`$TMPDIR/<name>-<date>/` elsewhere), never loose in the repo root, and stays there until
the report is written.

### A. Animation test box (pre-server pixels, no session)

The production render app with the DATs loaded directly — particles, routines, zone
geometry and weather by file id, with zero server state. **Requires a
`debug-animation_room` build** (`--features debug-animation_room`; the plugin is `#[cfg]`-gated
in `kuluu/src/view_native/mod.rs`). Launch with user + password only: passing a char name
auto-starts a session and plays the intro cutscene instead of opening the box. A non-empty
`ANIMTEST_AUTO` opens the box by itself, so the whole run is hands-free.

```bash
KULUU_WINDOW_HIDDEN=1 \
ANIMTEST_AUTO="zone,g141,g144,shot" \
ANIMTEST_ZONE_ID=<id> ANIMTEST_MZB_FILE_ID=<fid> ANIMTEST_WORLD_POS="0,0,0" \
ANIMTEST_SHOT_PATH=/c/tmp/<dated>/zone.png \
RUST_LOG=info ./target/release/kuluu.exe --server 127.0.0.1 play verilight 'TestPass!1234' \
  --mute > /dev/null 2> /c/tmp/<dated>/client.log &      # Unix: same, minus the .exe
```

- **Ask this build what it can drive.** Do not transcribe case names or environment variables from
  this file, from memory, or from someone's older capture — ask, and use what comes back:
  ```bash
  ./target/release/kuluu animtest --knobs      # no server, no install, no window; exits at once
  echo '{"query":"knobs"}' | nc 127.0.0.1 <FFXI_KEY_DRIVE-port>    # same answer from a running box
  ```
  The reply lists every fireable case name, every env knob with what it does and its value shape,
  and the auto-fire spacing. If it answers `"compiled_in": false`, **this build has no room**: rebuild
  with the features the answer names instead of guessing flags or concluding the box is broken.
- **Fire, then read the log.** Unknown case names warn and are dropped, so a typo'd list silently
  produces fewer captures than you asked for; `[animationtest]` lines show what actually ran.
- **Order matters more than speed**: cases fire on the fixed clock the report describes, so a long
  queue delays the case you care about, and a zone capture fired before MZB placement settles shows
  the wrong terrain — confirm in the log that the target file id's placements spawned before judging
  what you see.
- **The readback case is the pixel path**: Bevy reads back its own render target, so it lands while
  parked offscreen where no window capture can reach. The report names which knob sets the output
  file (unset means numbered default output). Include that case in the queue; if nothing lands,
  presentation is stalled → escalation ladder.

### B. Live session with pixels (offscreen window + key drive)

A real character in a real zone when the scene needs server state to be true — weather as
zoned, lighting at game time, particles on live MZB geometry. Launch exactly as §Render-health
runs but add `FFXI_KEY_DRIVE=:48198` (any free port; note it), then drive input as one JSON
line per TCP connection:

| Line | Effect |
|---|---|
| `{"query":"knobs"}` | what this build can drive: cases, env knobs and clock spacing — on a live session, where the offline `animtest --knobs` answer cannot see runtime state |
| `{"key":"printscreen"}` | in-app GPU readback → numbered `screenshot-<n>.png` **in the working directory**; move each into your dated folder immediately. Names accepted by `KeyMsg::resolve`: `printscreen`, `prtsc`, `prtscn` |
| `{"text":"//warp 123.4 -56.7"}` then `{"key":"enter"}` | dev commands take a **double** slash (`//warp`, `//whereami`, `//zones`, `//pathto`, `//endevent`); single `/cmd` is rejected with a suggestion |
| `{"text":"/shutdown"}` then `{"key":"enter"}` | GM single-slash commands (see §Cleanup) |

- **Positioning:** `//pathto <x> <y>` (wire coords) is the reliable mover; raw walking into
  props wastes cycles. `//zones` lists zonelines with rects — warping *into* a rect does not
  trigger the change; you must be standing in the source zone's rect. Verify a crossing in
  the map log, never by guessing.
- **Where am I:** login resumes at the char's saved position, so the login zone is not a
  spawn town — read it from `docker logs <map-container> --since 5m` (`Player <name> logging
  in to zone <id> (LoadChar)`), and use `//whereami` for live coords as an on-screen message
  (it does not reach client.log, so read it off a capture). Wire space is Z-up: `y` is ground.
- **A rejected command usually means InEvent**: right after zone-in a pending server event can
  block chat for ~20s (`"msg #2 para=0,0"` in the map log). Send `//endevent` + enter first.

### C. Scripted fallbacks when readback is not enough

```bash
# Unix only (it drives the agent socket): capture.sh retries with the socket owner's window raised
.agents/skills/verify/scripts/capture.sh <out.png> [socket-path] [client-pid]

# Windows-only: PrintWindow(PW_RENDERFULLCONTENT) on the parked window, client area cropped.
# Note the two script roots: verify's helpers live under .agents/skills/verify/scripts/, while
# cap-window.ps1 is in the repo-root scripts/ alongside checks.sh.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/cap-window.ps1 kuluu /c/tmp/<dated>/frame.png [wait-ms]
```

`cap-window.ps1` takes the first matching process with a window, so it is only safe when your
client is the sole one — check `tasklist | grep -ic kuluu` (Unix: `pgrep -c kuluu`) first.

- **A capture counts only if it shows content.** All-black or one flat colour is not
  evidence: retake once, and a second failure means presentation stalled → escalate rather
  than retry again or crop-flag-tweak. Repeated identical frames are *not* a symptom; static
  scenes legitimately repeat.
- **Nothing may pop up to check.** No desktop screen-grab, no "launch visible just to see",
  no example binary pointed at the zone — those put pixels on the user's display and are not
  evidence of an offscreen run either way.

## Provisioning throwaway chars manually

```bash
cargo run -p kuluu --features native-window -- provision <user> 'TestPass!1234'
cargo run -p kuluu --features native-window -- create-char <user> 'TestPass!1234' <Name> 1 1 0 1 1
docker exec server-database-1 mariadb -uxiadmin -ppassword xidb \
  -e "UPDATE chars SET pos_zone=230,pos_x=..,pos_y=..,pos_z=.. WHERE charname='<Name>';"
```

Fresh provisioned chars are a full E2E vehicle — zone changes and Mog House entry included.
The old "fresh chars get all c2s silently ignored" blocker was two client bugs, both fixed:
the c2s datagram header must carry the last subpacket's sync id (`session.rs::datagram_header_id`,
drift = server skips every subpacket) and the new-character intro cutscene rides the 0x00A
login packet (`decode::ZoneInEvent`) and must be answered with 0x05B or the char sticks
InEvent. If those symptoms return, check that header/sync invariant first.

## Cleanup and ownership

```bash
# Set OWNED_PID to the PID recorded when this run launched its client.
taskkill //PID "$OWNED_PID"      # Windows Git Bash
kill -TERM "$OWNED_PID"         # Unix
```

- **Prefer a clean disconnect** (MCP `disconnect`, the Unix agent socket, or client exit) over a
  kill: a hard kill leaves the map server holding the char for minutes and the next lobby login
  times out. `references/stack.md` owns that failure mode and its single-row repair — including why
  the `WHERE` clause is not optional.
- **GM `/shutdown` restarts the shared server.** It is not client cleanup. Use it only for
  an explicitly authorized server-restart test; record the interruption in the report.
- **Never kill a kuluu process you did not start.** The user plays on this machine too; confirm
  ownership before any taskkill.

## Gotchas

**Protocol surfaces (§1–§3)**

- `--headless` opens no window at all and runs no render pipeline: zero `kuluu_render` lines is
  the expected shape of a healthy session run, not a broken build.
- Raw stdio credentials are positional; env vars feed only the interactive launcher, which blocks
  on `Username:`.
- Coordinate swap bites twice — in commands *and* events, `y` is ground and `z` is vertical.
- colima dead / one-way UDP / login stuck at "Authenticating": the VM slept and virtiofs went
  stale → `colima restart`, then `docker start` the server containers. If login succeeds and map
  traffic goes silent, check the stack's DNAT sidecar before theorising about the client.

**Offscreen/pixel surfaces (§A–§C)**

- Char name in the play args on the test-box path = session auto-start + intro cutscene, not the box.
- `KULUU_WINDOW_HIDDEN=1` without `--mute` is audible BGM from an invisible window.
- A zone capture showing a different zone's terrain means `shot` fired before MZB placement settled.
- Particle origins far above the floor after a zone load mean the collision block was baked without
  its `world_pos` (`kuluu-render`/`dat_mzb` spawn path) — check there before blaming placement code.
