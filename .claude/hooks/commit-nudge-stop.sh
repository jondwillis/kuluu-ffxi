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
# Stop fires every turn end. Common knobs:
#   - Higher floor:
#       if [ "$file_count" -lt 3 ]; then exit 0; fi
#   - Time throttle (5-min min between nudges per session):
#       throttle_file="$snap_dir/${session_id}.last-nudge"
#       if [ -f "$throttle_file" ] && \
#          [ $(($(date +%s) - $(stat -f %m "$throttle_file"))) -lt 300 ]; then
#         exit 0
#       fi
#       touch "$throttle_file"
# ───────────────────────────────────────────────────────────────
if [ "$file_count" -lt 1 ]; then exit 0; fi

shown=$(printf '%s\n' "$session_lines" | head -20)
if [ "$file_count" -gt 20 ]; then
  shown="${shown}
... (+$((file_count - 20)) more)"
fi

msg=$(printf 'This session touched %s file(s) not yet committed. Consider grouping uncontroversial changes into a commit:\n%s' \
  "$file_count" "$shown")

jq -n --arg m "$msg" '{
  systemMessage: $m,
  hookSpecificOutput: { hookEventName: "Stop", additionalContext: $m }
}'
