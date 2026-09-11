#!/usr/bin/env bash
# Capture through the agent socket, retrying once with the socket owner's window raised.
# Usage: capture.sh <out.png> [socket-path] [client-pid]
# Black or stale captures are not visual evidence; see the native-video fallback
# in references/drive-gui.md. Pass the known socket when several clients are open.

set -euo pipefail

out="${1:?usage: capture.sh <out.png> [socket-path] [client-pid]}"
sock="${2:-}"
client_pid="${3:-}"

if [ -z "$sock" ]; then
  # $TMPDIR/ffxi-agent.pid goes stale across the cargo-wrapper -> binary re-exec,
  # so resolve from the live socket files instead.
  sock=$(ls -t "${TMPDIR}"ffxi-agent-*.sock 2>/dev/null | head -1 || true)
fi
[ -z "$sock" ] && { echo "capture.sh: no agent socket found in \$TMPDIR" >&2; exit 1; }

mkdir -p "$(dirname "$out")"

shoot() {
  rm -f "$out"
  python3 - "$sock" "$out" <<'PY'
import json, socket, sys, time
sock_path, out = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(5.0)
s.connect(sock_path)
s.sendall((json.dumps({"cmd": "screenshot", "path": out}) + "\n").encode())
time.sleep(0.3)
s.close()
PY
  # The write is async (GPU readback -> map_async -> disk), so wait for the file
  # rather than assuming it landed.
  for _ in $(seq 1 40); do
    [ -s "$out" ] && break
    sleep 0.25
  done
  [ -s "$out" ] || { echo "capture.sh: $out never appeared — is this a GUI session?" >&2; return 1; }
}

# Exits 0 if the frame has content, 2 if it is blank.
lit_check() {
  python3 - "$out" <<'PY'
import sys
try:
    from PIL import Image
except ImportError:
    print("capture.sh: Pillow missing, skipping blank-frame guard", file=sys.stderr)
    sys.exit(0)
im = Image.open(sys.argv[1]).convert("L")
hist = im.histogram()
frac = sum(hist[4:]) / max(1, sum(hist))
print(f"capture.sh: {sys.argv[1]} {im.size[0]}x{im.size[1]} lit={frac:.1%}")
sys.exit(2 if frac < 0.01 else 0)
PY
}

shoot || exit 1
if ! lit_check; then
  echo "capture.sh: blank GPU readback; occlusion is one possible cause." >&2
  if [ -z "$client_pid" ] && [[ "$(basename "$sock")" =~ ^ffxi-agent-([0-9]+)\.sock$ ]]; then
    client_pid="${BASH_REMATCH[1]}"
  fi
  if ! [[ "$client_pid" =~ ^[0-9]+$ ]] || [ "$client_pid" -le 0 ]; then
    echo "capture.sh: cannot identify this socket's client; pass its PID as argument 3." >&2
    exit 2
  fi
  client_comm=$(ps -p "$client_pid" -o comm= 2>/dev/null || true)
  if [ "$(basename "$client_comm")" != "kuluu" ]; then
    echo "capture.sh: PID $client_pid is not a live kuluu client; refusing to raise it." >&2
    exit 2
  fi
  echo "capture.sh: raising only PID $client_pid once; FOCUS WILL BLIP." >&2
  prev=$(osascript -e 'tell application "System Events" to get unix id of first process whose frontmost is true' 2>/dev/null || true)
  if [ -n "$client_pid" ]; then
    osascript -e "tell application \"System Events\" to tell (first process whose unix id is $client_pid)" \
      -e 'set frontmost to true' \
      -e 'if exists window 1 then' \
      -e 'set value of attribute "AXMinimized" of window 1 to false' \
      -e 'perform action "AXRaise" of window 1' \
      -e 'end if' -e 'end tell' >/dev/null 2>&1 || true
    sleep 1.2
    capture_status=0
    shoot || capture_status=$?
    if [[ "$prev" =~ ^[0-9]+$ ]] && [ "$prev" != "$client_pid" ]; then
      osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $prev) to true" >/dev/null 2>&1 || true
    fi
    [ "$capture_status" -eq 0 ] || exit "$capture_status"
  fi
  lit_check || {
    echo "capture.sh: still blank after raising; check console/window state, then use" >&2
    echo "            the native window-video fallback in references/drive-gui.md." >&2
    echo "            Do NOT cite this file as visual evidence." >&2
    exit 2
  }
fi
