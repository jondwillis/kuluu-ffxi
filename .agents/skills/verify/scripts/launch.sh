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

# Give the window a moment to exist, then hand focus back. Doing this once after
# the window appears is enough — the client never re-activates itself.
sleep 6
if [ -n "$prev" ] && [ "$prev" != "kuluu" ]; then
  osascript -e "tell application \"System Events\" to set frontmost of (first process whose name is \"$prev\") to true" >/dev/null 2>&1 || true
fi

# Zone-in takes a while; wait for map traffic rather than a fixed sleep.
for _ in $(seq 1 60); do
  kill -0 "$client_pid" 2>/dev/null || { echo "launch.sh: client died — see $log" >&2; exit 1; }
  if grep -aq "sub_opcodes" "$log" 2>/dev/null; then break; fi
  sleep 2
done

sock=$(grep -ao "/var/folders[^ ]*ffxi-agent-[0-9]*\.sock" "$log" | tail -1 || true)
[ -z "$sock" ] && { echo "launch.sh: no agent socket in $log" >&2; exit 1; }

echo "launch.sh: pid=$client_pid sock=$sock (focus returned to ${prev:-unknown})"
echo "$sock"
