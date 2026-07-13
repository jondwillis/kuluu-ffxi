---
name: verify
description: Runtime-verify client changes against the local LSB dev stack — headless JSON drive, throwaway chars, env gotchas
---

# Verifying ffxi-client changes against the local LSB stack

## Stack up

LSB runs as docker containers (`server-{database,connect,world,search,map}-1`)
under colima. The external drive sleeping kills colima; symptoms are a dead
lobby or one-way UDP.

```bash
colima status || colima start
docker ps -a | grep server-          # map/world often Exited (255) after VM sleep
docker start server-map-1 server-world-1
docker logs server-map-1 -f          # wait for "The map-server is ready to work"
```

Map UDP must stay published as `127.0.0.1:54230/udp` (lima forwarding race).

## Throwaway account/char

```bash
cargo run -p ffxi-client --features native-window -- provision <user> 'TestPass!1234'
cargo run -p ffxi-client --features native-window -- create-char <user> 'TestPass!1234' <Name> 1 1 0 1 1
docker exec server-database-1 mariadb -uxiadmin -ppassword xidb \
  -e "UPDATE chars SET pos_zone=230,pos_prevzone=230,pos_x=..,pos_y=..,pos_z=.. WHERE charname='<Name>';"
```

## Headless drive (protocol surface)

`play --headless` works in the `native-window` build (reuses artifacts). Drive
it via a FIFO; stdout is the JSON event stream, stderr the tracing log:

```bash
mkfifo $D/in; (exec 3>$D/in; sleep 900 & wait) &   # hold the write end open
cargo run -q -p ffxi-client --features native-window -- \
  play <user> 'TestPass!1234' <Name> --headless < $D/in > $D/events.jsonl 2> $D/client.log &
echo '{"cmd":"move","x":..,"y":..,"z":..,"heading":0}' > $D/in
echo '{"cmd":"request_zone_change","line_id":812805498}' > $D/in   # zmr0 Southern San d'Oria
```

Commands: `AgentCommand` serde (`{"cmd":"..."}`), events: `{"type":"..."}`.
`--agent-listen`/`FFXI_AGENT_LISTEN` exposes the same protocol on a unix socket
(works for the GUI session too). Positional creds, NOT env vars — env vars are
only read by the interactive launcher.

## Gotchas

- Killing the client leaves a server-side ghost session for 2–5 min: the next
  lobby login times out (`lpkt_next_login: no response in 20s`). Wait for map's
  `cleanupSessions` log line, then `DELETE FROM accounts_sessions;`.
- **Fresh lobby-created chars get ALL substantive c2s silently ignored** by this
  LSB build (0x05E zonelines, 0x0B5 chat produce no response/echo/log) while
  keepalive + entity flow stays healthy. Reproduced across client commits, so
  it's server-side char/account state (suspect: unfired new-char intro cutscene
  after a DB pos teleport). Long-lived chars don't have this. Until root-caused,
  use a real char for zone-change E2E; ephemeral chars are fine for login,
  spawn-stream, and inventory observation.
- `.claude` session shells share the host; `docker`/`colima` need no sudo.
- Reactor (auto zoneline trigger, follow, auto-attack) is NOT wired in the
  `play --headless` main.rs path — send `request_zone_change` explicitly.
