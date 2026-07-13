# Headless drive: MCP, raw stdio, live tests

Three ways to a protocol-observable session, strongest first.

## 1. ffxi-mcp standalone (preferred)

Spawns the full supervisor→reactor→session pipeline and exposes MCP
tools/resources over stdio. This is the only headless path with the reactor
(goals, keepalive, event auto-dismiss) and the only one with event-driven
waits instead of log polling.

```bash
cargo build -p ffxi-mcp          # binary at target/debug/ffxi-mcp
FFXI_USER=... FFXI_PASS=... FFXI_CHAR=... FFXI_SERVER=127.0.0.1 target/debug/ffxi-mcp
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

## 2. Raw stdio (`play --headless`)

Zero extra deps; JSON commands on stdin, typed JSON events on stdout, tracing
on stderr. **No reactor** — send explicit commands only.

```bash
D=$(mktemp -d); mkfifo $D/in; (exec 3>$D/in; sleep 900 & wait) &   # hold write end open
cargo run -q -p ffxi-client --features native-window -- \
  play <user> '<pass>' <CharName> --headless < $D/in > $D/events.jsonl 2> $D/client.log &

echo '{"cmd":"move","x":164.9,"y":164.8,"z":-5.5,"heading":64}' > $D/in
echo '{"cmd":"request_zone_change","line_id":812805498}' > $D/in    # zmr0, S. San d'Oria
```

- Commands are `AgentCommand` serde: `{"cmd":"snake_case", ...}` (state.rs).
  Events are `{"type":"snake_case", ...}`.
- Credentials are **positional args**, not env — env vars only feed the
  interactive launcher, which will otherwise block on a `Username:` prompt.
- Coordinate space in commands/events: `x` = native x, `y` = ground (native z),
  `z` = vertical (native y).
- Zoneline ids are the fourcc as LE u32 — look them up in
  `vendor/server/sql/zonelines.sql` (comments name each line).

## 3. Live integration tests (canonical layer proofs)

These drive the real client against the real server and **self-skip when no
server is reachable** — they are runtime verification harnesses, not CI
re-runs. Use them to bisect which layer is broken before hand-driving:

```bash
cargo test -p ffxi-client --test play_lifecycle -- --nocapture   # auth→lobby→map→InZone→disconnect (~3s)
cargo test -p ffxi-client --test zone_change    -- --nocapture   # GM !zone → reconnect → re-zone-in
cargo test -p ffxi-client --test agent_session  -- --nocapture   # full MCP-driven session (transport floor)
RESTART_MAP_SERVER=1 cargo test -p ffxi-client --test disconnect_recovery -- --nocapture  # destructive, opt-in
```

They use the `EphemeralChar` fixture (`tests/common/mod.rs`): isolated
account + char stamped into MariaDB, gmlevel set before first login. If a
manual flow fails where the matching test passes, diff your flow against the
fixture's — that delta is the bug or the blocker.

## Provisioning throwaway chars manually

```bash
cargo run -p ffxi-client --features native-window -- provision <user> 'TestPass!1234'
cargo run -p ffxi-client --features native-window -- create-char <user> 'TestPass!1234' <Name> 1 1 0 1 1
docker exec server-database-1 mariadb -uxiadmin -ppassword xidb \
  -e "UPDATE chars SET pos_zone=230,pos_x=..,pos_y=..,pos_z=.. WHERE charname='<Name>';"
```

Remember the fresh-char c2s blocker (SKILL.md §Character strategy): good for
login/spawn/inventory observation, currently no good for zoning or chat.
