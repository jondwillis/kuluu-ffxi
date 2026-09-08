#!/usr/bin/env bash
# Focus-free GUI capture (kuluu-wwwv). Sends `screenshot` over the agent socket,
# so Bevy reads back the render target on the GPU: no window raise, no keystroke,
# no Screen Recording permission, and the human keeps whatever they were doing.
#
#   capture.sh <out.png> [socket-path]
#
# The one condition the GPU path cannot escape: macOS stops producing drawables
# for a FULLY occluded window, so the readback comes back solid black. That
# failure is silent — a valid PNG of nothing — so this script asserts the frame
# has content. Partially visible is enough; focus is not needed.
#
# If the frame IS blank, it raises the client once, re-captures, and hands focus
# straight back. Correct evidence beats zero disruption, and a ~1s blip is far
# cheaper than a black PNG being cited as proof — but it is logged loudly so you
# know the human was interrupted, and it only happens when the window got buried.

set -euo pipefail

out="${1:?usage: capture.sh <out.png> [socket-path]}"
sock="${2:-}"

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
  echo "capture.sh: blank frame — client window is fully occluded, so macOS stopped" >&2
  echo "            rendering it. Raising it once to get a real frame; FOCUS WILL BLIP." >&2
  pid=$(pgrep -f "^target/(release|debug)/kuluu" | head -1 || true)
  prev=$(osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>/dev/null || true)
  if [ -n "$pid" ]; then
    osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $pid) to true" >/dev/null 2>&1 || true
    sleep 1.2
    shoot || exit 1
    if [ -n "$prev" ] && [ "$prev" != "kuluu" ]; then
      osascript -e "tell application \"System Events\" to set frontmost of (first process whose name is \"$prev\") to true" >/dev/null 2>&1 || true
    fi
  fi
  lit_check || {
    echo "capture.sh: still blank after raising — is the console locked, or the app hidden?" >&2
    echo "            Do NOT cite this file as evidence." >&2
    exit 2
  }
fi
