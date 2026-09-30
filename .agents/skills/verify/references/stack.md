# LSB dev stack: bring-up and environment gotchas

The stack is five docker containers under colima:

| Container | Role | Host port |
|---|---|---|
| `server-connect-1` | auth (TCP/TLS) + lobby | 54231 / 54001 |
| `server-world-1` | world / char-list | internal |
| `server-search-1` | auction search | internal |
| `server-map-1` | map (UDP + Blowfish) | 54230/udp |
| `server-database-1` | MariaDB (`xidb`) | 3306 |

## Bring-up and tear-down

`scripts/lsb-stack.sh` owns the lifecycle — use it rather than hand-driving
colima and docker:

```bash
scripts/lsb-stack.sh up        # colima + containers, waits for map-server ready
scripts/lsb-stack.sh status    # VM, container states, idle-teardown countdown
scripts/lsb-stack.sh down      # stop the containers and the colima VM
```

`down` takes the VM down too — stopping only the containers leaves colima
holding its whole CPU/memory allocation, which is the cost worth reclaiming.
The price is a ~20-30s VM boot on the next `up` instead of a bare
`docker start`.

**Stop the stack when you're done verifying.** `up` arms an idle reaper that
tears it down after 30 minutes (`LSB_STACK_IDLE_SECS`) with no client
attached, and the Stop hook `.agents/hooks/stop.d/45-stack.sh` tears it down
when a session settles — but both are backstops, not a reason to leave it up.
Neither fires while a `kuluu`/`kuluu-mcp` process is alive, so a session you
are still driving is never pulled out from under you. `LSB_STACK_AUTOSTOP=off`
disables the hook; `scripts/lsb-stack.sh touch` pushes the idle deadline out
for long work the process check can't see.

What `up` does, if you need to drive it by hand: `colima start`, then
`docker start` the five containers (often `Exited (255)` after VM sleep), wait
for `The map-server is ready to work` in `docker logs server-map-1`, then
confirm the auth listener with `nc -z 127.0.0.1 54231`.

DB access for fixtures/inspection:

```bash
docker exec server-database-1 mariadb -uxiadmin -ppassword xidb -e "..."
```

## Failure modes (all observed, all documented fixes)

- **colima dead / one-way UDP / login stuck at "Authenticating"** — the
  external Sidecar drive slept and virtiofs went stale. Fix: `colima restart`,
  then `docker start server-map-1 server-world-1`. Symptom variant: map UDP
  exchanges the login burst then goes silent both ways.
- **Map UDP must stay published `127.0.0.1:54230/udp`** — publishing on
  0.0.0.0 hits a lima forwarding race. Check with `docker port server-map-1`.
- **Ghost sessions after killing a client** — the next lobby login times out
  (`lobby lpkt_next_login (view): server did not respond within 20s`). The map
  server holds the char for 2–5 min. Prefer a clean `disconnect` (MCP tool /
  client exit) over `kill` to avoid this entirely; back-to-back relaunches
  otherwise land in a lockout loop costing ~2 min per retry.

  Either wait for the server's own `cleanupSessions` log line, or clear the one
  stale row for the character you are driving:

  ```bash
  docker exec server-database-1 mariadb -uxiadmin -ppassword xidb \
    -e "DELETE FROM accounts_sessions WHERE charid=<charid>;"
  ```

  Get `<charid>` from `SELECT charid FROM chars WHERE charname='<name>';`
  (Verilamp is 17455719 on this machine's dev DB). Keep the `WHERE` — other
  sessions on this stack are not yours to drop, and a whole-table delete is a
  destructive action that needs the user's say-so.
- **Zone changes silently ignored for a manually provisioned fresh char** —
  see SKILL.md §Character strategy. Not an env problem; don't restart the
  stack over it.

## Map-server watchdog restarts

If chat/actions stop responding while the GUI still renders, inspect the map
log for `INACTIVITY WATCHDOG HAS TRIGGERED` and a fresh server start. A local
run has hit the 2000ms watchdog in `CStatusEffectContainer::TickEffects` /
`CZoneEntities::mobTick`; its clients kept rendering after losing the server.
Discard affected command-response claims, wait for server readiness, and
reconnect only the test session. Record the tombstone and investigate load
rather than disabling the watchdog or restarting another person's client.

## Server-side introspection

- `docker logs server-map-1 --since 5m` — LoadChar / IncreaseZoneCounter /
  cleanupSessions / GM traces at debug level; packet-validator failures log as
  `Invalid <name> packet from <char>`.
- Authoritative handler behavior lives in `vendor/server/src/map/packets/c2s/`
  — when the server does something surprising, read the handler before
  theorizing.
