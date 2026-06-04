#!/usr/bin/env bash
# Stop hook: compare current dirty state to the SessionStart
# snapshot. Lines present now but absent at start = work *this*
# session produced. If any, nudge the user to group uncontroversial
# changes into a commit.
#
# Note: the Stop event fires every turn end, not only at session
# exit. The TUNE-ME block below is the place to add a throttle or
# raise the minimum file-count threshold if it feels too chatty.

set -euo pipefail

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty')
[ -z "$cwd" ] && cwd="$PWD"
[ -z "$session_id" ] && exit 0

# Loop guard: if we're already in a stop-hook continuation (the agent is
# running *because* a previous Stop block re-invoked it), do NOT block
# again — that would spin forever. Give intelligence exactly one shot per
# stop, then let it stop cleanly.
stop_active=$(printf '%s' "$payload" | jq -r '.stop_hook_active // false')
[ "$stop_active" = "true" ] && exit 0

git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1 || exit 0

snap_dir="${TMPDIR:-/tmp}/claude-commit-nudge"
snap_file="$snap_dir/${session_id}.porcelain"
[ -f "$snap_file" ] || exit 0  # no baseline → can't tell what's session work

current=$(git -C "$cwd" status --porcelain 2>/dev/null || true)
[ -z "$current" ] && exit 0

# comm -23 needs sorted inputs; --porcelain lines are stable.
session_lines=$(comm -23 \
  <(printf '%s\n' "$current"   | sort -u) \
  <(printf '%s\n' "$(cat "$snap_file")" | sort -u) \
  | grep -v '^$' || true)

[ -z "$session_lines" ] && exit 0

file_count=$(printf '%s\n' "$session_lines" | grep -c . || true)

# ─── TUNE-ME ───────────────────────────────────────────────────
# Stop fires every turn end. The block below re-invokes the agent so it
# can decide whether to commit — without the user sending a message. To
# keep that from happening on *every* turn while the agent legitimately
# defers, throttle to once per session per window. Knobs:
#   - Higher file floor:  if [ "$file_count" -lt 3 ]; then exit 0; fi
#   - Throttle window (seconds):
THROTTLE_SECS=300
if [ "$file_count" -lt 1 ]; then exit 0; fi

throttle_file="$snap_dir/${session_id}.last-nudge"
if [ -f "$throttle_file" ] && \
   [ $(($(date +%s) - $(stat -f %m "$throttle_file"))) -lt "$THROTTLE_SECS" ]; then
  exit 0
fi
touch "$throttle_file"
# ───────────────────────────────────────────────────────────────

shown=$(printf '%s\n' "$session_lines" | head -20)
if [ "$file_count" -gt 20 ]; then
  shown="${shown}
... (+$((file_count - 20)) more)"
fi

# Block the stop and hand the decision back to the agent. The `reason`
# is injected as a fresh prompt — the agent wakes up, judges whether the
# changes form one or more coherent, uncontroversial commits, and either
# commits them (scoped `git add <paths>`, never `-A` — sibling sessions
# may have unrelated edits in this tree) or stops if the work is mid-flight.
reason=$(printf 'Stop-hook checkpoint: this session produced %s uncommitted file(s):\n%s\n\nDecide — do these form one or more coherent, uncontroversial commits? If so, commit now with scoped `git add <paths>` (NEVER `git add -A`; other sessions may have unrelated edits) and a clear message. If the work is mid-flight or not yet coherent, do nothing and stop. This checkpoint fires at most once per %ss, so deferring is fine.' \
  "$file_count" "$shown" "$THROTTLE_SECS")

jq -n --arg r "$reason" '{ decision: "block", reason: $r }'
