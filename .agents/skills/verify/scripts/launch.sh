#!/usr/bin/env bash
# Launch a GUI verification session without taking over the user's machine.
#
#   launch.sh <logfile> [extra kuluu play args...]
#
# Runs the RELEASE binary by default. The dev profile is Cranelift + no opt and
# renders zone-in at well under 1fps with multi-second frame spikes, which makes
# anything short-lived (a fade, a cast bar, a hit flash) unsamplable and turns
# every drive into a wall-clock sink. Override with FFXI_VERIFY_PROFILE=debug
# when the change under test needs debug_assertions or a dev-only feature.
#
# Passes --unfocused --mute by default (drop --mute by exporting FFXI_VERIFY_SOUND=1
# when the change under test is audio). Then restores whatever app was frontmost:
# macOS activates a newly launched app at the process level, which winit's
# `focused: false` does not suppress, so without this a launch yanks the user out
# of full-screen video. Restoring afterwards is the only lever that works from
# outside the client — Bevy builds the winit event loop itself and exposes no
# macOS ActivationPolicy hook.
#
# Prints the resolved agent socket path on success.

set -euo pipefail

log="${1:?usage: launch.sh <logfile> [play args...]}"
shift || true

: "${FFXI_VERIFY_USER:=verilight}"
: "${FFXI_VERIFY_PASS:=TestPass!1234}"
: "${FFXI_VERIFY_CHAR:=Verilamp}"

flags=(--unfocused)
[ "${FFXI_VERIFY_SOUND:-0}" = "1" ] || flags+=(--mute)

profile="${FFXI_VERIFY_PROFILE:-release}"
bin="target/$profile/kuluu"
if [ ! -x "$bin" ]; then
  cargo_flag=$([ "$profile" = release ] && echo " --release")
  echo "launch.sh: $bin missing — build it with:" >&2
  echo "  cargo build -p kuluu --features native-window$cargo_flag" >&2
  exit 1
fi
echo "launch.sh: using $bin" >&2

prev=$(osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>/dev/null || true)

rm -f "$log"
"$bin" --agent-listen auto play "${flags[@]}" "$@" \
  "$FFXI_VERIFY_USER" "$FFXI_VERIFY_PASS" "$FFXI_VERIFY_CHAR" > "$log" 2>&1 &
client_pid=$!
ready=0
cleanup_failed_launch() {
  if [ "$ready" != 1 ]; then
    if kill -0 "$client_pid" 2>/dev/null; then
      python3 - "$log" "$client_pid" <<'PYTHON_CLEANUP' || true
import json
from pathlib import Path
import re
import socket
import sys
import time

log_path, client_pid = sys.argv[1:]
try:
    pattern = r"^agent socket listening on (.+/ffxi-agent-" + client_pid + r"\.sock)$"
    paths = re.findall(pattern, Path(log_path).read_text(errors="replace"), re.MULTILINE)
    if paths:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(2.0)
            connection.connect(paths[-1])
            connection.sendall(b'{"cmd":"disconnect"}\n')
            deadline = time.monotonic() + 2.0
            pending = b""
            while time.monotonic() < deadline:
                connection.settimeout(max(0.01, deadline - time.monotonic()))
                chunk = connection.recv(65536)
                if not chunk:
                    break
                pending += chunk
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    if json.loads(line).get("type") == "disconnected":
                        sys.exit(0)
except (OSError, ValueError):
    pass
PYTHON_CLEANUP
      kill "$client_pid" 2>/dev/null || true
      for _ in {1..10}; do
        kill -0 "$client_pid" 2>/dev/null || break
        sleep 0.2
      done
      kill -0 "$client_pid" 2>/dev/null && kill -KILL "$client_pid" 2>/dev/null || true
    fi
    wait "$client_pid" 2>/dev/null || true
  fi
}
trap cleanup_failed_launch EXIT
trap 'exit 1' INT TERM

# Give the window a moment to exist, then hand focus back. Doing this once after
# the window appears is enough — the client never re-activates itself.
sleep 6
if [ -n "$prev" ] && [ "$prev" != "kuluu" ]; then
  osascript -e "tell application \"System Events\" to set frontmost of (first process whose name is \"$prev\") to true" >/dev/null 2>&1 || true
fi

# A snapshot response proves zone entry even when debug packet logs are disabled.
sock=$(python3 - "$log" "$client_pid" "${FFXI_VERIFY_TIMEOUT_SECS:-120}" <<'PYTHON'
import json
import os
from pathlib import Path
import re
import socket
import sys
import time

log_path, client_pid, timeout = sys.argv[1:]
client_pid = int(client_pid)
deadline = time.monotonic() + float(timeout)
socket_line = re.compile(r"^agent socket listening on (.+/ffxi-agent-" + str(client_pid) + r"\.sock)$", re.MULTILINE)
last_state = "agent socket not announced"
while time.monotonic() < deadline:
    try:
        os.kill(client_pid, 0)
    except ProcessLookupError:
        sys.exit(f"launch.sh: client died before entering zone; see {log_path}")
    matches = socket_line.findall(Path(log_path).read_text(errors="replace"))
    if matches:
        sock_path = matches[-1]
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(min(2.0, max(0.01, deadline - time.monotonic())))
                connection.connect(sock_path)
                connection.sendall(b'{"cmd":"snapshot"}\n')
                response_deadline = min(deadline, time.monotonic() + 2.0)
                pending = b""
                while time.monotonic() < response_deadline:
                    connection.settimeout(max(0.01, response_deadline - time.monotonic()))
                    chunk = connection.recv(65536)
                    if not chunk:
                        last_state = "agent socket closed before zone readiness"
                        break
                    pending += chunk
                    while b"\n" in pending:
                        line, pending = pending.split(b"\n", 1)
                        try:
                            event = json.loads(line)
                        except (ValueError, UnicodeError):
                            continue
                        if event.get("type") == "diagnostics":
                            stage = event.get("diagnostics", {}).get("stage")
                            if stage == "in_zone":
                                print(sock_path)
                                sys.exit(0)
                            last_state = f"diagnostics stage={stage}"
                        elif event.get("type") == "stage_changed":
                            last_state = f"stage={event.get('stage')}"
                        elif event.get("type") == "disconnected":
                            sys.exit(f"launch.sh: client disconnected before entering zone; see {log_path}")
                        elif event.get("type") == "error":
                            last_state = "agent reported an error"
        except OSError as error:
            last_state = f"agent socket unavailable ({type(error).__name__})"
    time.sleep(min(0.2, max(0.0, deadline - time.monotonic())))
sys.exit(f"launch.sh: timed out waiting for in_zone ({last_state}); see {log_path}")
PYTHON
)
ready=1

echo "launch.sh: pid=$client_pid sock=$sock (focus returned to ${prev:-unknown})"
echo "$sock"
