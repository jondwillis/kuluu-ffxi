#!/usr/bin/env bash
# SessionStart hook: snapshot the working-tree dirty state for this
# session_id and nudge the user about any *inherited* uncommitted
# work — i.e. files that were already dirty before this session
# began. The Stop counterpart diffs against this snapshot to flag
# work this session itself produced.
#
# Snapshot path: $TMPDIR/claude-commit-nudge/<session_id>.porcelain
# Stays through the session; gets garbage-collected by OS tmp cleanup.

set -euo pipefail

payload=$(cat)

session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty')
[ -z "$cwd" ] && cwd="$PWD"
[ -z "$session_id" ] && exit 0

# Only operate inside a git repo. Quiet otherwise.
git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1 || exit 0

porcelain=$(git -C "$cwd" status --porcelain 2>/dev/null || true)

snap_dir="${TMPDIR:-/tmp}/claude-commit-nudge"
mkdir -p "$snap_dir"
printf '%s' "$porcelain" > "$snap_dir/${session_id}.porcelain"

# Clean tree, nothing to say.
[ -z "$porcelain" ] && exit 0

file_count=$(printf '%s\n' "$porcelain" | grep -c . || true)

# ─── TUNE-ME ───────────────────────────────────────────────────
# Skip the nudge entirely when conditions don't warrant it.
# Defaults: any pending file triggers a nudge.
# Examples you might add:
#   - if [ "$file_count" -lt 3 ]; then exit 0; fi
#   - if printf '%s\n' "$porcelain" | grep -qvE '\.md$|\.claude/'; then : ; else exit 0; fi
# ───────────────────────────────────────────────────────────────
if [ "$file_count" -lt 1 ]; then exit 0; fi

# Cap the listing so the user sees signal, not a wall.
shown=$(printf '%s\n' "$porcelain" | head -20)
if [ "$file_count" -gt 20 ]; then
  shown="${shown}
... (+$((file_count - 20)) more)"
fi

msg=$(printf 'Inherited %s uncommitted file(s) from before this session. Consider grouping uncontroversial changes into a commit before piling more on:\n%s' \
  "$file_count" "$shown")

jq -n --arg m "$msg" '{
  systemMessage: $m,
  hookSpecificOutput: { hookEventName: "SessionStart", additionalContext: $m }
}'
