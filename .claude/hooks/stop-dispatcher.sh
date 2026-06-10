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
stop_active=$(printf '%s' "$payload" | jq -r '.stop_hook_active // false')
session_id=$(printf '%s' "$payload" | jq -r '.session_id // "unknown"')

# Loop guard — a bounded continuation DEPTH, not a one-shot.
#
# We deliberately do NOT bail on every stop_hook_active stop. That older
# rule gave each check exactly one shot per continuation chain: if a
# high-priority check transiently missed on the first stop — e.g.
# 10-ask-question read the transcript a beat before the final assistant
# text block (the question) had landed on disk — the lower checks fired,
# the chain continued, and the next stop (stop_hook_active=true) was
# swallowed whole. The question was then fully present but never
# re-examined, silently defeating the stop.d/ precedence order.
#
# Per-check sig_changed (see stop-lib.sh) is the real same-content loop
# guard: a check never re-fires for content it already fired on, so a
# settled chain converges to all-pass on its own. This counter is only a
# backstop against a check that fires on ever-CHANGING content. Reset on
# a natural stop; bump once per continuation.
depth_dir="${TMPDIR:-/tmp}/claude-stop-dispatch"
mkdir -p "$depth_dir"
depth_file="$depth_dir/${session_id}.depth"
if [ "$stop_active" = "true" ]; then
  depth=$(( $(cat "$depth_file" 2>/dev/null || echo 0) + 1 ))
else
  depth=0
fi
printf '%s' "$depth" > "$depth_file"
[ "$depth" -ge 8 ] && exit 0

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
