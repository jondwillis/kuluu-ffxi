# Operating the FFXI agent harness

Operator-facing runbook for verifying the harness end-to-end against a live
LandSandBoat-family server. The LLM-facing playbook is in `CLAUDE.md` /
`AGENTS.md`; this doc is for the human driving the agent on a stage.

## 1. Prerequisites

A running LSB stack. The dev `docker-compose` brings up:

| Container          | Role                       | Port (host) |
|--------------------|----------------------------|-------------|
| `server-connect-1` | login (TCP/TLS) + lobby    | 54231 / 54001 |
| `server-world-1`   | world / char-list          | (internal)  |
| `server-search-1`  | auction search             | (internal)  |
| `server-map-1`     | map (UDP / Blowfish)       | 54230       |
| `server-database-1`| MariaDB                    | 3306        |

Quick check:

```bash
docker ps --format '{{.Names}}\t{{.Status}}'
nc -zv 127.0.0.1 54231   # auth listener should accept
```

If the stack is fresh, give it ~10 s to settle before driving traffic.

## 2. Build

```bash
cargo build -p ffxi-mcp
```

The MCP binary lands at `target/debug/ffxi-mcp`. Integration tests locate it
via `current_exe` walk-up; if you skip this step, `agent_session` and
`disconnect_recovery` panic with an explicit instruction.

If your working tree has the parallel-session view-3d WIP applied, the
binary target may not compile — use `cargo build -p ffxi-mcp --bin ffxi-mcp`
or stash the WIP first.

## 3. Unit tests

```bash
cargo test -p ffxi-proto                 # 18 protocol tests
cargo test -p ffxi-client --lib          # 48 client lib tests
cargo test -p ffxi-mcp                   # 5 notifier-filter tests
```

The `--lib` filter on `ffxi-client` skips the binary target — useful while
the parallel session has `src/main.rs` mid-refactor.

## 4. Integration tests (live LSB stack)

All live tests skip cleanly when no server reachable on `SERVER_HOST:AUTH_PORT`
(defaults `127.0.0.1:54231`).

### `play_lifecycle` — auth → lobby → map → InZone → disconnect

```bash
cargo test -p ffxi-client --lib --test play_lifecycle -- --nocapture
```

~3 s wall time. Validates the bare session actor without supervisor/MCP
wrapping. Use this first when diagnosing whether failures are in the
session layer or above it.

### `zone_change` — `!zone N` → reconnect → re-zone-in

```bash
cargo test -p ffxi-client --lib --test zone_change -- --nocapture
```

Exercises the GM `!zone` command (requires `gmlevel ≥ 1`, which the fixture
sets). Validates Blowfish key rotation on zone transition.

### `agent_session` — full MCP-driven session

```bash
cargo test -p ffxi-client --lib --test agent_session -- --nocapture
```

~3.5 s wall time. Drives `ffxi-mcp` over JSON-RPC stdio:
`initialize` → `tools/list` → `resources/list` → `resources/subscribe scene://current`
→ wait-for-InZone → read `scene://current` → `tools/call snapshot` → expect
`notifications/resources/updated` → `tools/call disconnect`.

This is the **transport conformance** floor. Does NOT exercise:

- BtTargetID / aggro detection (no mobs in scene)
- Party packets (solo)
- `/tell` (no recipient)
- `RequestZoneChange` (no zoneline traversal)
- Reactor goals (`Follow` / `Engage` / `PathTo` never issued)

### `disconnect_recovery` — destructive, opt-in

```bash
RESTART_MAP_SERVER=1 cargo test -p ffxi-client --lib --test disconnect_recovery \
    -- --nocapture
```

Drives `docker restart -t 0 server-map-1` mid-session and asserts the
supervisor recovers. **Affects other tests using the same stack** — run
serialized.

**Currently failing** against committed behavior — see §8.

## 5. Driving an LLM harness manually

Set credentials, point an MCP-capable harness at the binary, drive the
session interactively.

```bash
# Use real credentials, not the EphemeralChar fixture.
export FFXI_USER='your_account'
export FFXI_PASS='your_password'
export FFXI_CHAR_ID=12345678        # u32 from chars.charid
export FFXI_CHAR='YourCharName'
export FFXI_SERVER=127.0.0.1
export RUST_LOG=info,ffxi_client=info,ffxi_mcp=debug
```

Then:

* **Claude Code**: `cd ffxi-agent && claude` (auto-discovers `.mcp.json`).
* **OpenCode**: `cd ffxi-agent && opencode` (same `.mcp.json`).
* **MCP Inspector** (UI for poking at the server):
  `npx @modelcontextprotocol/inspector ./target/debug/ffxi-mcp`

The inspector is the fastest way to confirm tools/resources surface
correctly without involving an LLM.

### 5a. Harness compatibility matrix

All supported harnesses speak MCP over stdio with the same `.mcp.json`.
None require special transport flags or wrapper scripts — the binary
target is identical regardless of who's driving.

| Harness         | Config file        | Transport | Env interpolation | Auto-discovery | Notifications |
|-----------------|--------------------|-----------|-------------------|----------------|---------------|
| Claude Code     | `.mcp.json`        | stdio     | `${VAR}`          | yes (CWD)      | yes           |
| OpenCode        | `.mcp.json`        | stdio     | `${VAR}`          | yes (CWD)      | yes           |
| pi.dev          | `.mcp.json`        | stdio     | `${VAR}`          | yes (CWD)      | yes           |
| MCP Inspector   | n/a (CLI args)     | stdio     | shell             | n/a            | yes           |

Cross-harness invariants:

- Tool surface — 13 tools, same shape (`follow`, `engage`, `path_to`,
  `cancel`, `chat`, `tell`, `request_zone_change`, `end_event`,
  `snapshot`, `cast`, `weaponskill`, `job_ability`, `use_item`,
  `bank_when_full`, `disconnect`).
- Resource surface — 5 resources (`scene://current`, `party://members`,
  `diagnostics://session`, `goal://current`, `inventory://current`).
- Notifications — `notifications/resources/updated` fires on the
  `AgentEvent`s gated in `ffxi-mcp/src/main.rs::uris_for_event`.
- Working directory — start the harness from `ffxi-agent/` so the
  `.mcp.json` resolves; that file points to `../Cargo.toml -p ffxi-mcp`,
  which compiles the MCP binary against the workspace at the repo root.
  `cargo test -p ffxi-client` etc. run from the repo root, not `ffxi-agent/`.

Per-harness gotchas that have been observed:

- **OpenCode** may surface env-interpolation errors if your shell
  doesn't export the referenced variables before the harness launches.
  Confirm with `env | grep FFXI_` before starting.
- **MCP Inspector** doesn't auto-discover `.mcp.json`; pass the binary
  path directly, and set env vars in your shell first.
- **All three** assume one MCP server per harness — running two
  harnesses against the same Phoenix container with the same `FFXI_USER`
  produces "char already logged in" errors from the lobby.

## 5b. Watching the agent — native viewer HUD

The `ffxi-client` binary opens a Bevy-windowed 3D scene of the FFXI world
plus an operator HUD that mirrors what the harness is currently driving.
Useful when an LLM is on the wheel and you want a glanceable picture of
what it's deciding.

```bash
cargo run -p ffxi-client --bin ffxi-client -- play
```

(Direct mode — pass `--user`, `--password`, `--char` to skip the launcher.
Without them, the windowed launcher prompts.)

What's painted:

| Region | What it shows | When it changes |
|---|---|---|
| Top stage bar | Auth/zoning stage, character, zone | Stage transitions |
| Top-left **agent HUD** | Current reactor goal, color-coded state pill, last-reconnect age | Every `ReactorGoalChanged`; recon clock counts up continuously |
| Top-right **LLM badge** | Pulse dot, latency sparkline (last 32), p50/p99, paired/solo count | Every `LlmDecision` (notification fired or tool dispatched) |
| Right-side roster | Party HP/MP/TP per member | Party packets (`0x0DD` / `0x0DF`) |
| Bottom-left chat | Recent chat lines | Every `ChatLine` |

Reading the LLM badge:

* **Cyan ◉ (bright)** — the harness dispatched a tool within the last
  200 ms in response to a notification we fired. Healthy round-trip.
* **Cyan ●** — same as above, fading over ~2 s.
* **White ●** — tool dispatched without a preceding notification. The
  LLM is acting on its own initiative (e.g. periodic re-poll), not
  reacting to a `notifications/resources/updated`.
* **Gray ●** — no decision in the last 600 ms, log is settling.
* **Dark ●/○** — log empty or stale.

The sparkline is window-max scaled to the visible 32-sample slice, so a
single big outlier compresses the rest. Read p50/p99 for absolute
numbers.

If the agent HUD shows `[IDLE]` when you expect it to be working, the
LLM has either not issued a goal yet or sent `cancel`. The reactor
still keepalives but won't auto-attack/follow/path.

## 6. Stage 7 verification scenarios

### 6a. Autonomous goal — 60-minute farming loop

Goal definition for the LLM (paste into the harness's first message):

> Farm crawler cocoons in West Sarutabaruta for 60 minutes; bank to mog
> house when inventory hits 30/30.

Mid-run validation: ~30 minutes in, force a disconnect:

```bash
docker restart -t 0 server-map-1
```

The supervisor must reconnect and resume the persisted goal from
`~/.config/ffxi-mcp/goal.json` (or `$FFXI_MCP_GOAL_PATH`).

Watch for:

* `INFO ffxi_client::supervisor: supervisor.attempt.start attempt=2 replaying_goal=true`
* `INFO ffxi_client::supervisor: supervisor.reconnected attempt=2 downtime_ms=…`

### 6b. Co-play goal — agent-as-healer

Run a second character (a melee) under your manual control, in the same
party as the agent. Goal for the LLM:

> Follow the party leader; cure them when their HP drops below 75%; cure
> on `/tell @cure`. Do not engage mobs.

Validation:

* Issue `/tell @cure` from the leader; agent's first reaction should appear
  in the party's chat log within ~1.5 s.
* Pull mob aggro onto the agent; agent should emit an `EngagedBy` event
  and the harness should re-prioritise.
* Walk away from the agent; reactor's `Follow` should keep stepping
  toward you until in-range.

## 7. Reading the latency instrumentation

Tracing events fire at three layers, each at the appropriate level so the
defaults don't flood:

| Event                          | Level | Fields                                        |
|--------------------------------|-------|-----------------------------------------------|
| `reactor.tick`                 | trace | `elapsed_us`, `cmds_emitted`                  |
| `supervisor.attempt.start`     | info  | `attempt`, `replaying_goal`                   |
| `supervisor.attempt.end`       | info  | `attempt`, `duration_ms`, `outcome`           |
| `supervisor.reconnected`       | info  | `attempt`, `downtime_ms`                      |
| `mcp.tool_dispatch`            | debug | `kind`, `elapsed_us`, `ok`                    |
| `mcp.resource_read`            | debug | `uri`, `elapsed_us`                           |

To profile reactor ticks:

```bash
RUST_LOG=info,ffxi_client::reactor=trace cargo run -p ffxi-mcp …
```

For aggregation into p50/p95/p99, pipe through `jq` (events are
key=value, not JSON; convert with `tracing-subscriber` JSON formatter
if you need machine parsing — out of scope here).

Plan budgets for reference:

* Reactor decisions ≤ 250 ms p99
* MCP tool dispatch ≤ 50 ms p99 (excluding LLM time)
* Reconnect downtime ≤ 8 s p95 on transient drops — see §8

## 8. Known gaps

### Hard-crash recovery is bounded by 60 s UDP-silence detection

`session.rs:606` declares disconnect after 60 s without any inbound server
packet. UDP gives no socket-level "connection lost" signal, so this is the
only mechanism. On a hard map-server crash (`docker restart`), expect:

* ~60 s for the supervisor to notice
* ~5–10 s for re-auth + re-zone
* total ≥ 65 s recovery

The plan target (≤ 8 s p95) was achievable only if we had TCP keepalive on
a separate channel — we don't. Two paths to reconcile:

1. Lower the silence threshold (e.g. to 15 s = 15 missed 1 Hz keepalives).
   Trades robustness against short server stalls for faster recovery.
2. Keep the 60 s threshold and update the plan target to acknowledge the
   UDP floor: ≤ 8 s p95 *transient*, ≤ 90 s p95 *hard crash*.

`tests/disconnect_recovery.rs` asserts the 30 s budget and currently fails
loudly so this gap stays visible.

### Live-calibration caveats (also in `CLAUDE.md`)

* **Heading math** (`reactor::heading_toward`) — internally consistent
  (n=0/e=64/s=128/w=192) but may need a constant offset to match server
  expectations. Test by issuing `move` north and watching the character
  in another client.
* **`RequestZoneChange`** — packet builder unit-tested; server-side
  acceptance not yet observed in a live run.
* **Party-packet decode** (`0x0DD` / `0x0DF`) — schema-checked but real
  party traffic has not exercised the merge logic.
* **BtTargetID offset** — `body[40..44]` per `Phoenix/src/.../char_update.cpp:187`,
  but unvalidated against live aggro packets.
* **`/tell` layout** — `unknown00` / `unknown01` fields modelled per
  `Phoenix/src/map/packets/c2s/0x0b6_chat_name.h`; first real-server use
  will validate.

### Parallel-session WIP

`ffxi-client/src/main.rs` may be mid-refactor by the parallel 3D dashboard
session. If `cargo test -p ffxi-client` fails on the bin target with a
`view3d::run` arity mismatch, use `--lib --test <name>` to skip the bin.
