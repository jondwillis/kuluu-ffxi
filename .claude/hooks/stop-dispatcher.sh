#!/usr/bin/env bash
# Stop dispatcher: a thin ordered runner over the sub-checks in stop.d/.
#
# Why a dispatcher: when several Stop hooks each return `block` they
# compete on one Stop event, and the loud ones crowd out the quiet one —
# e.g. a pending ask-question never gets re-posed because commit/comment
# blocks land first. Funnelling through one runner makes precedence
# explicit (filename order in stop.d/) and emits at most ONE block per
# cycle. Lower-priority checks surface on later cycles once the top one
# resolves; only the firing check records its signature (see stop-lib.sh).
#
# Adding/removing/reordering a check = drop or rename a file in stop.d/;
# no edit here. Each check is a standalone script (stdin = payload, exit
# 0 = pass, exit 10 = fire with the reason on stdout) — independently
# testable: `echo "$payload" | .claude/hooks/stop.d/20-commit.sh; echo $?`.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
FIRE=10

payload=$(cat)

# Loop guard: never block twice in one stop continuation — one shot per
# stop, then let the agent stop cleanly.
stop_active=$(printf '%s' "$payload" | jq -r '.stop_hook_active // false')
[ "$stop_active" = "true" ] && exit 0

for check in "$here"/stop.d/*.sh; do
  [ -e "$check" ] || continue  # literal glob when stop.d/ is empty
  reason=$(printf '%s' "$payload" | bash "$check")
  rc=$?
  if [ "$rc" -eq "$FIRE" ]; then
    jq -n --arg r "$reason" '{ decision: "block", reason: $r }'
    exit 0
  fi
  [ "$rc" -ne 0 ] && printf 'stop-dispatcher: %s exited %s\n' "$(basename "$check")" "$rc" >&2
done

exit 0
