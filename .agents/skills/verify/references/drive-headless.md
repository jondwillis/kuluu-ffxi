# Headless drive — canonical recipes (single source of truth)

Recipes for running Kuluu without a visible window where the host supports
reliable readback. Choose the Windows or Unix entries for the actual host;
macOS visual verification uses the companion native-window workflow.
Companion: `.agents/skills/verify/SKILL.md` (canonical verify recipes + evidence recording).

## Credentials

Use the local throwaway accounts or fixture provisioning in `../SKILL.md`
("Character strategy"). Never ask for or log the user's real credentials.
The documented local test credentials are safe for local-stack verification.

## Options menu — pick by what you're verifying

| You need | Option | Section |
|---|---|---|
| **No pixels** — server output only: wire protocol, zoning, cutscene flow, chat, entity spawn | B2 raw stdio `play --headless` (JSON events on stdout) or B1 MCP | §B |
| **Pixels, pre-server** — particles/routines in the animationtest box, zone/weather load by file id | A AnimationTest box (hidden window). Loads MZB geometry + particles with no server state | §A |
| **Pixels, live session** — real character in a real zone: zone lighting, weather as zoned, anything needing server state | D key-drive on a hidden live session (printscreen GPU readback) | §D |
| **No server at all** — standalone VFX test box (worm + Hume, dam-cascade buttons) | C `kuluu_noserver_tester`. Only when the user tells you to use it; it may be outdated for future fixes | §C |

### Windows options list (this machine)

1. **Build**: repo-root release `.bat` (untracked by design) — release + full local feature
   batch, exe synced to repo root. Never an ad-hoc `cargo build -p kuluu --features <subset>`.
2. **Render box**: Surface A launch with `KULUU_WINDOW_HIDDEN=1` (§A).
3. **Pixels from a hidden window**: in-app GPU readback — the `shot` case for surface A, the
   `printscreen` keybind over FFXI_KEY_DRIVE for live sessions (§D). Both work at zero-size
   windows; `scripts/cap-window.ps1` only when the window has a client area.
4. **Session, no pixels**: raw stdio three-command recipe (§B2) or MCP standalone (§B1).
5. **No server**: `cargo run -p kuluu_noserver_tester --features native-window` (§C).
6. **Kill**: `taskkill //F //IM kuluu.exe`; verify with `tasklist | grep -ic kuluu`.

### Unix / other-OS options list

1. **Build**: `cargo build -p kuluu --features native-window --release`; run
   `./target/release/kuluu`. Add only the opt-in features the check exercises.
2. **Render box**: on macOS, use the visible, muted, unfocused native-window
   workflow in `drive-gui.md`; warn before launch. Hidden Metal readbacks can
   stay entirely black while the simulation advances. Other Unix hosts may use
   Surface A with `KULUU_WINDOW_HIDDEN=1`; inspect the capture before trusting it.
3. **Pixels**: on macOS, use `scripts/capture.sh` with the exact test socket/PID.
   Its one-time raise can recover a black readback; if it cannot, use the
   window-only video fallback in `drive-gui.md`. Check changing poses or scene
   content against the command trace. `cap-window.ps1` is Windows-only.
4. **Session, no pixels**: raw stdio with `mktemp -d` + pipe holder (§B2), MCP standalone
   (§B1), TCP injection via `nc` (§B2).
5. **No server**: `cargo run -p kuluu_noserver_tester --features native-window` (§C).
6. **Kill**: `pkill -f kuluu`; verify with `pgrep -c kuluu`.

## 0. The rules that keep runs from wrecking the desktop

1. **Build with the repo-root release `.bat`** (Windows; untracked by design) — release +
   full local feature batch, exe synced to repo root. Never an ad-hoc `cargo build -p kuluu --features <subset>`: a
   half-feature binary is not what gets verified and behavior can differ. On other OSes,
   use the native release build above and record the features actually tested.
2. **Use the host-appropriate capture path.** Windows Surface A/D uses
   `KULUU_WINDOW_HIDDEN=1`; macOS visual checks follow `drive-gui.md` and warn
   before opening or raising the test window. Surface B needs no window.
3. **Always mute** launches (`--mute`): a hidden run still decodes and plays BGM/SFX.
4. **Kill the process when done.** Windows: `taskkill //F //IM kuluu.exe`. Other OSes:
   `pkill -f kuluu`. Check first — never fire a test if one is already running
   (`tasklist | grep -i kuluu` / `pgrep -a kuluu`). If the user may be playing, confirm which
   processes are yours before killing anything.
5. **Run from the repo root** — game files resolve from `vendor/game-files` relative to CWD.

## A. Surface A: AnimationTest box (pre-server, pixels)

The box lives in `AppPhase::Launcher`. Launch with user+password only (no char name): that
stops at character select, where the box auto-opens when `ANIMTEST_AUTO` is set. Passing a
char name auto-starts a session and plays the intro cutscene instead — never do that here.

### Stack check

```bash
docker ps --format "{{.Names}}\t{{.Status}}"                # all server containers Up
# host ports (container ports are remapped on this machine):
#   auth 53232   data 53231   view 54001
```

If the stack is down: `bash scripts/lsb-stack.sh up` (manual recovery in the untracked local stack notes).

### Launch (Windows, Git Bash)

```bash
cd <repo root>
KULUU_WINDOW_HIDDEN=1 \
ANIMTEST_AUTO="zone,weather,nhit,chit,dhit,mobnhit,mobchit,respawn,hi26,sb00" \
FFXI_MAP_LOCAL_PORT=47500 WGPU_ADAPTER_NAME=NVIDIA \
nohup ./kuluu.exe --server 127.0.0.1 --auth-port 53232 --data-port 53231 \
  play <user> '<pass>' --mute > /dev/null 2> client.log &
```

Other OSes: same env vars, `./target/release/kuluu` (or the synced repo-root binary), no
`WGPU_ADAPTER_NAME` pinning. The port numbers are this machine's docker remap — read them
from `docker port <connect container>` (name it via `docker ps`), not from memory.

- `ANIMTEST_AUTO` — comma-separated case names fired on a fixed clock with no input: first
  fire ~4s after app start, then one every 7s. The full list runs ~95s; pass only the cases
  you need (e.g. `zone,g141,g144`). Case names:
  `nhit chit dhit mobnhit mobchit respawn levelup g141 g144 hit1full hi26 sb00 zone weather shot`
  (`kuluu/src/view_native/animation_test_scene.rs::case_from_name`).
- Env overrides for the LoadZone case: `ANIMTEST_ZONE_ID`, `ANIMTEST_MZB_FILE_ID`,
  `ANIMTEST_WORLD_POS="x,y,z"` (zone geometry lands at absolute mzb_to_bevy(native) coords
  when ZERO), `ANIMTEST_HOUR=<h>` (pin VanaClock for night/midday captures),
  `ANIMTEST_CAM="px,py,pz,tx,ty,tz"` (re-frame the box camera).

### Reading the log

Everything of interest is an `[animationtest]` line on stderr:

| Line | Meaning |
|---|---|
| `ANIMTEST_AUTO: ...` | queue parsed at startup |
| `loaded: worm=... hume=[face=... sword=...]` | actor load requests out |
| `check ok: face=...(14) ...` / `ERROR: slot N (...) unreadable` | per-part mesh-buffer check before draw |
| `drawn on player: N mesh parts (all checked parts, sword included)` | post-spawn drawn count vs expected |
| `AUTO fired <case>` / `case: <case>` | a case started |
| `route <routine> spawned particle generator <gen> mesh <mesh> ... origin=(x,y,z)` | per-generator placement — the target-focus evidence |
| `wire lost: worm` / `wire lost: hume` | an actor was despawned mid-run (zone load, sync sweep) |
| `worm dead — respawning before hit` | auto-respawn guard fired |

Pass criteria for a grounded scene: particle origins at torso height (`y ≈ 0.6` worm-side /
`1.1` target-facing), worm-side generators at `x < 0`, target-facing at `x > 0`; zero
`wire lost` lines; no `ERROR:` lines.

### Capturing screenshots (hidden window, never on screen)

Two paths, in order of reliability:

1. **In-app `shot` case (preferred)** — Bevy reads back the render target itself, so it
   works even when the hidden window is zero-size (which is what `KULUU_WINDOW_HIDDEN=1`
   produces): add `shot` to `ANIMTEST_AUTO` and set `ANIMTEST_SHOT_PATH=C:/tmp/name.png`.
   It fires on the AUTO clock (~4s + 7s per preceding case), so order matters: put `zone`
   (or whatever you're framing) before `shot`, and remember zone placement takes longer than
   one tick — if the frame is empty, add more cases between them or re-run.
2. **`scripts/cap-window.ps1`** (Windows only) — `PrintWindow(PW_RENDERFULLCONTENT)` on the
   buried window; needs a non-zero client area (`bounds=0x0` = "zero-size window", use path 1):

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/cap-window.ps1 kuluu artifacts/verify/<name>.png
```

- Fire it a few seconds after the `[animationtest] AUTO fired <case>` line you want to
  frame. Particle lifetimes are authored in 30 fps frames, so capture within ~1s of the
  fire for mid-burst shots; zone/weather shots can wait until placement settles (the log's
  `queued ... placements` / weather lines).
- Output is the window client area at its configured resolution (default 1280x800,
  `FFXI_WINDOW_SIZE=WxH` to change). A black frame does not establish why
  capture failed. On macOS, follow `drive-gui.md` rather than repeatedly
  retrying hidden readbacks.
- These hidden-window recipes apply where readback works. For macOS, use the
  native test window and capture fallback above; capture only that window,
  restore focus after raising it, and stop only task-owned clients.

### Kill

```bash
taskkill //F //IM kuluu.exe        # Windows   (other OSes: pkill -f kuluu)
tasklist | grep -ic kuluu          # must be 0 (pgrep -c kuluu elsewhere)
```

## B. Surface B: session headless (no pixels, server output)

**`--headless` is session-only: it runs NO render pipeline.** No Bevy app, no MZB geometry,
no particles — only protocol/session state (that's why `client.log` carries zero
`kuluu_render` lines). A live session answers "what does the server send around here", not
"what is drawn". For any render-layer question use Surface A or D.

### B1. kuluu-mcp standalone (preferred for sessions)

Spawns the full supervisor→reactor→session pipeline and exposes MCP tools/resources over
stdio. The only headless path with the reactor (goals, keepalive, event auto-dismiss) and
event-driven waits instead of log polling.

```bash
cargo build -p kuluu-mcp          # binary at target/debug/kuluu-mcp
FFXI_USER=... FFXI_PASS=... FFXI_CHAR=... FFXI_SERVER=127.0.0.1 target/debug/kuluu-mcp
```

Drive it as an MCP server over stdio (`.mcp.json` — `ffxi-agent/.mcp.json` is the canonical
config). High-value calls:

- `wait_for_event {kinds, timeout_ms}` — block until `zone_changed` / `entity_upserted` /
  `connected` fires. Use instead of polling.
- `read_resource scene://current` — entities, zone, self state as JSON.
- `read_resource diagnostics://session` — seq/sync counters, net health.
- `request_zone_change {line_id}` — zoneline trigger (char must be standing in the rect;
  move there first with `path_to`).
- `snapshot`, `chat`, `cast`, `engage`, `follow`, `disconnect` — full vocabulary in
  `ffxi-agent/instructions/playbook.md`.

`FFXI_ATTACH=auto` instead attaches to an already-running client — that is the GUI path,
not headless.

### B2. Raw stdio (`play --headless`)

Zero extra deps; JSON commands on stdin, typed JSON events on stdout, tracing on stderr.
Uses the reactor's explicit agent profile (goal commands and fishing automation behave as
MCP); no supervisor/reconnect layer or event-driven MCP waits.

Verified working recipe (Windows / Git Bash, this machine's stack). **Three separate
commands** — do not chain them:

```bash
# 1) setup its own command: D=/c/tmp/agent; rm -rf $D; mkdir -p $D; mkfifo $D/in

# 2) launch (prebuilt repo-root exe from the release .bat, ports via `docker port <connect container>`):
(exec 3>$D/in; sleep 900) &   # hold the fifo write end open for the session's lifetime
./kuluu.exe --server 127.0.0.1 --auth-port 53232 --data-port 53231 \
  play <user> '<pass>' <CharName> --headless --mute < $D/in > $D/events.jsonl 2> $D/client.log &

# 3) drive / read, each its own command:
sleep 12; head -c 3000 $D/events.jsonl    # connected -> zone_changed -> entity_upserted
echo '{"cmd":"move","x":164.9,"y":164.8,"z":-5.5,"heading":64}' > $D/in
echo '{"cmd":"request_zone_change","line_id":812805498}' > $D/in    # zmr0, S. San d'Oria
```

Simpler stdin holder (any OS, no mkfifo): `(sleep 1800 | ./kuluu.exe ... --headless) > events.jsonl 2> client.log &`
— the pipe's write end stays open for `sleep`'s lifetime; EOF ends the session cleanly.

Other OSes: same three steps with `./target/release/kuluu`;
`mktemp -d` is fine for `$D`; kill with `pkill -f kuluu`.

- Commands are `AgentCommand` serde: `{"cmd":"snake_case", ...}`; events are `AgentEvent`:
  `{"type":"snake_case", ...}` (`kuluu-session/src/state.rs`).
- Credentials are **positional args**, not env — env vars only feed the interactive
  launcher, which otherwise blocks on a `Username:` prompt. Ask the user first (§Credentials).
- Coordinate space in commands/events: `x` = native x, `y` = ground (native z),
  `z` = vertical (native y).
- Zoneline ids are the fourcc as LE u32 — look them up in
  `vendor/server/data/zones/<zone>/zone.yaml` zonelines.
- `--headless` opens **no window at all** (Surface B never needs `KULUU_WINDOW_HIDDEN`,
  which is a Surface A/D thing); still pass `--mute`.
- Use the prebuilt repo-root `./kuluu.exe` from the release .bat, not an ad-hoc
  `cargo run`: a half-feature debug binary is not what gets verified (rule 0).
- **Git Bash precedence gotcha**: `A && B & C` backgrounds the whole chain. If mkfifo sits
  in the same command as the launch, kuluu's `< $D/in` redirect can race fifo creation and
  die with "No such file or directory" before it ever connects. Setup and launch must be
  separate commands (verified failure mode).
- **Injecting commands into a running session**: once stdin is held by `sleep`, write to the
  agent TCP listener instead — launch with `FFXI_AGENT_LISTEN=:48199` and send one JSON line
  per connection:
  - Windows: `powershell -NoProfile -Command "$t=New-Object Net.Sockets.TcpClient('127.0.0.1',48199); $s=$t.GetStream(); $b=[Text.Encoding]::UTF8.GetBytes('{\"cmd\":\"move\",...}'); $s.Write($b,0,$b.Length); Start-Sleep -Milliseconds 300; $t.Close()"`
    (from Git Bash the `$` vars get eaten — put it in a .ps1 file and use `-File`).
  - Unix: `echo '{"cmd":"move",...}' | nc 127.0.0.1 48199`.
- Reading what the server sends around you: every nearby entity is an `entity_upserted`
  event; self is the one with `kind:"pc"` (its entity id varies per login). Compute
  distance from self's `pos` over all upserts to see exactly who/what the server placed in
  the area — zone fixtures (lamps, torches) are NOT entities, they come from the MZB and
  only show up in Surface A/D runs.

### B3. Live integration tests (canonical layer proofs)

In `kuluu-session/tests/`. They drive the real client against the real server and
**self-skip when the auth port or xidb is unreachable** (`tests/common/mod.rs`) — runtime
verification harnesses, not CI re-runs. Use them to bisect which layer is broken before
hand-driving.

| Test | Proves | Run |
|---|---|---|
| `play_lifecycle.rs` | auth→lobby→map→InZone→disconnect (~3s) | `cargo test -p kuluu-session --test play_lifecycle -- --nocapture` |
| `zone_change.rs` | GM `!zone` → reconnect → re-zone-in | `cargo test -p kuluu-session --test zone_change -- --nocapture` |
| `agent_session.rs` | full MCP-driven session (transport floor); spawns `target/debug/kuluu-mcp` — rebuild it first | `cargo test -p kuluu-session --test agent_session -- --nocapture` |
| `disconnect_recovery.rs` | map-server restart mid-session; destructive, opt-in | `RESTART_MAP_SERVER=1 cargo test -p kuluu-session --test disconnect_recovery -- --nocapture` |
| `event_503_live.rs` | end-to-end playback of the SSD new-character cutscene: cues in authored order, input-gated frames, onEventFinish rewards (item 536 + setPos to the gate); also self-skips when no FFXI install can be opened | `cargo test -p kuluu-session --test event_503_live -- --nocapture` |
| `auction_search_live.rs` | AH search-server smoke: cast a category and history over TCP SEARCH_PORT; no map session needed | `cargo test -p kuluu-session --test auction_search_live -- --nocapture` |
| `delivery_box_live.rs` | server-side delivery-box flow driven directly via the agent channel | `cargo test -p kuluu-session --test delivery_box_live -- --nocapture` |
| `action_dispatch.rs` | offline: subpacket layouts (cast/weaponskill/job-ability/item-use) match the phoenix structs | `cargo test -p kuluu-session --test action_dispatch` |

They use the `EphemeralChar` fixture (`tests/common/mod.rs`): an isolated account + char
stamped into MariaDB, gmlevel set before first login. If a manual flow fails where the
matching test passes, diff your flow against the fixture's — that delta is the bug or the
blocker. Gotcha: when the accounts AUTO_INCREMENT outruns the fixture's sentinel accid
scheme, the lobby rejects the char select ("mismatched character name" in connect logs)
and the test dies at the 0x02 ack step.

## C. No-server option: kuluu_noserver_tester

Standalone VFX test box (worm + sworded Hume, dam-cascade buttons, no session layer). Use it
**only when the user tells you to**; it may be outdated for future fixes — check its log
output against current behavior before trusting a verdict.

```bash
cargo run -p kuluu_noserver_tester --features native-window   # Windows: same; needs the DATs (FFXI_DAT_PATH or default install)
```

It opens its own window with an on-screen log panel + stderr lines prefixed
`[kuluu_noserver_tester]`; `ANIMTEST_AUTO`-style case names fire the same dam0 cases as
Surface A. No server, no login — it loads a scene straight from the DATs.

## D. Surface D: live session with pixels (hidden window + key drive)

A real character zoned into a real zone, full render pipeline, zero visible windows. This
is the surface for questions that need **server state** to be true — zone lighting at game
time in a specific tunnel, weather as the server sends it, particles on live MZB — where
the pre-server box (§A) cannot reproduce the scene.

### Launch (Windows, Git Bash; ask the user for credentials first)

```bash
cd <repo root>
KULUU_WINDOW_HIDDEN=1 WGPU_ADAPTER_NAME=NVIDIA FFXI_KEY_DRIVE=:48198 \
nohup ./kuluu.exe --server 127.0.0.1 --auth-port 53232 --data-port 53231 \
  play <user> '<pass>' <CharName> --mute > /dev/null 2> C:/tmp/<dated-folder>/client.log &
```

Other OSes: same env vars, `./target/release/kuluu`, no adapter pinning. Pick any free
port for `FFXI_KEY_DRIVE` (example uses 48198); note it — every recipe below targets it.
The character logs in at its **saved position**, so the login zone is whatever he last
stood in — confirm via §D "checking zone/location", do not assume a spawn town.

### The key-drive protocol (one JSON line per TCP connection)

All input goes over `FFXI_KEY_DRIVE` as one JSON object per connection, written with
`printf ... > /dev/tcp/127.0.0.1/<port>` from Git Bash (PowerShell mangles the JSON):

| Line | Effect |
|---|---|
| `{"text":"//warp 123.4 -56.7"}` then `{"key":"enter"}` | type + submit a **dev command** — dev commands take a DOUBLE slash (`//warp`, `//whereami`, `//lights`, `//zones`, `//pathto`, `//endevent`). Single-slash `/cmd` is rejected with a suggestion. |
| `{"text":"/shutdown"}` then `{"key":"enter"}` | GM single-slash commands (server restart; see clean exit below). |
| `{"key":"printscreen"}` | in-app GPU readback → `screenshot-N.png` in the repo root, N incrementing. Works on a zero-size hidden window — this is THE pixel path for live sessions (`cap-window.ps1` does not work here: it needs a client area). Move each capture into your dated tmp folder before viewing; re-capture overwrites nothing (N keeps counting). |
| `{"key":"enter"}` / any other key name | plain key press (see `KeyMsg::resolve` in `kuluu/src/view_native/key_drive.rs` for the accepted names, incl. F1–F12 and printscreen). |

Typed characters are deferred one frame on release so Bevy sees a clean
press→release; you do not need to space them out. Chat input requires `InputMode::World`;
right after a zone-in a pending server event can block chat for ~20s — send
`{"text":"//endevent"}` + enter first, and treat a rejected command
(map log: `"msg #2 para=0,0"`) as the InEvent symptom.

### Walking to a place

- `//pathto <x> <y>` (wire coords, double slash) navmesh-walks there — **the reliable
  mover**. Raw walking into props/canyons wastes cycles; use pathto for every reposition.
- Zonelines: `//zones` lists them with rect coords. Warping INTO a rect does NOT trigger the
  zone change — you must be in the source zone and stand inside the rect (pathto there,
  wait). Verify the crossing in the map log (§D location check), not by guessing.

### Checking zone / location

- **Login zone + position**: `docker logs <map-container> --since 5m | grep -i <char>` (name it via
  `docker ps`) — look for
  `Player <name> logging in to zone <id> (LoadChar)` and the `IncreaseZoneCounter` line.
- **Live position on screen**: `//whereami` prints `self_pos: x=.. y=.. z=..  zone=<id>` as
  an on-screen system message — read it from a printscreen capture (it does not go to
  client.log). Wire space, Z-up: `y` is ground.

### Clean exit (do this, in order)

1. `{"text":"/shutdown"}` + enter from the session (GM account) — restarts map/world and
   releases every held session; no ghost rows left for the next login.
2. Wait ~35s; confirm `docker ps` shows all containers Up again.
3. Then `taskkill //F //IM kuluu.exe` (or let the disconnect kill it) so nothing of yours is
   holding a client slot while the user logs in.

A plain taskkill without /shutdown leaves a ghost session that makes the next lobby login
time out for minutes (§7). If you had to hard-kill, say so — the user may prefer to reset
the stack themselves.

### Captures and artifacts

Dated tmp folder per investigation (e.g. `C:/tmp/lampcap-YYYYMMDD/`): client.log there,
every printscreen capture moved in with a name that says what it shows
(`AFTER_vanilla_arch_1.png`, `flicker_A.png`). Keep raw frames until the report is written;
quote what you observed from them inline.

## 5. Evidence

- Surface A: `client.log` `[animationtest]` lines (table in §A) — quote them inline; pixels
  via the `shot` case / `scripts/cap-window.ps1` (§A capture), saved under `artifacts/verify/`.
- Surface B: `events.jsonl` (stdout JSON events), `client.log` (stderr tracing),
  `scene://current` snapshots, and the map-server log (`docker logs <map container> --since 5m` —
  LoadChar / cleanupSessions / `Invalid <name> packet from <char>` validator failures).
- Surface D: dated tmp folder with client.log + named printscreen captures (§D); map-log
  lines for zone/position; quote the observed lines inline.
- Keep the raw captures until the report is delivered; quote the observed lines inline.
- Recording for the stop-hook verify gate:
  `.agents/skills/verify/scripts/record-evidence.sh --verdict pass --summary "<what was
  observed>" --artifact <path>` (rules in the verify skill, §Recording evidence).

## 7. Gotchas

**Both surfaces**

- **Ghost sessions after killing a client** — the next lobby login times out ("server did
  not respond within 20s"); the map server holds the char for 2–5 min. Prefer a clean
  `disconnect` (MCP tool / agent TCP / client exit) over `kill`. Otherwise wait for the
  server's own `cleanupSessions` line, or clear the one stale row:
  `DELETE FROM accounts_sessions WHERE charid=<charid>;` (keep the `WHERE` — other sessions
  on this stack are not yours to drop, and a whole-table delete is destructive).
- **colima dead / one-way UDP / login stuck at "Authenticating"** — the VM slept and
  virtiofs went stale: `colima restart`, then `docker start` the server containers.
- **Map UDP return path** — s2c UDP replies are kept alive by a DNAT sidecar container in
  the local stack (see the untracked stack notes); if login succeeds and then map traffic
  goes silent, check that before theorizing about the client.

**Surface A**

- Char name in the play args = session auto-start + intro cutscene, not the box. User+pass
  only.
- `KULUU_WINDOW_HIDDEN=1` without `--mute` = audible BGM from an invisible window.
- The hidden window is **zero-size**: `cap-window.ps1` reports "zero-size window" — use the
  in-app `shot` case for pixels (§A).
- A zone capture that shows a different zone's terrain: the `shot` fired before MZB
  placement settled (the AUTO clock is fixed at ~4s + 7s/case; a full zone load takes
  longer). Re-order/re-run with more lead time, and confirm in the log that the target
  file_id's placements spawned before judging what you see.
- Particle origins at `y ≈ 40` after a zone load meant the collision block was baked
  without its `world_pos` (fixed in dat_mzb `spawn_mzb_overlay`) — if it ever returns, that
  is the first thing to check before blaming placement code.

**Surface B**

- **Raw stdio credentials are positional** — env vars only feed the interactive launcher
  (which blocks on a `Username:` prompt).
- **Coordinate swap** — in commands and events `y` is ground and `z` is vertical.
- **Session-only, no render** — see §B; do not expect geometry/particle log lines here.
- **Ending a driven session**: send `/shutdown` (GM) to restart the server and release every
  held session, then wait ~35s for docker to bring it back up before logging in again. A
  plain `taskkill` leaves a ghost session that the lobby rejects for minutes ("server did
  not respond within 20s", "no valid sessionHash").

**Surface D**

- **Login zone is the saved position**, not a spawn town — read it from the map log before
  assuming where you are (§D location check).
- **PrintScreen is the only pixel path** on a hidden live window; `cap-window.ps1` fails
  with "zero-size window". Numbered `screenshot-N.png` files land in the repo root — move
  them out immediately.
- **Dev commands are double-slash, GM single-slash.** A rejected command usually means
  InEvent state — `//endevent` + enter clears it (map log shows `"msg #2 para=0,0"`).
- **Never kill a kuluu.exe you did not start** — the user plays on this machine too;
  confirm ownership (`tasklist`, who launched it) before any taskkill.
