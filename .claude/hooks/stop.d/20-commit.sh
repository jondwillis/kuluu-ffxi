#!/usr/bin/env bash
# Stop sub-check (priority 20): lines dirty now but absent from the
# SessionStart baseline = work this session produced. If any, nudge the
# agent to group uncontroversial changes into a commit. Baseline written
# by commit-nudge-start.sh.
#
# Contract: see stop-lib.sh. Exit 0 = pass; fire = exit 10 + reason.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=../stop-lib.sh
. "$here/../stop-lib.sh"
load_payload

git -C "$CWD" rev-parse --git-dir >/dev/null 2>&1 || exit 0

snap_file="${TMPDIR:-/tmp}/claude-commit-nudge/${SESSION_ID}.porcelain"
[ -f "$snap_file" ] || exit 0  # no baseline → can't tell what's session work

current=$(git -C "$CWD" status --porcelain 2>/dev/null || true)
[ -z "$current" ] && exit 0

# comm -23 needs sorted inputs; --porcelain lines are stable.
session_lines=$(comm -23 \
  <(printf '%s\n' "$current" | sort -u) \
  <(printf '%s\n' "$(cat "$snap_file")" | sort -u) \
  | grep -v '^$' || true)
[ -z "$session_lines" ] && exit 0

file_count=$(printf '%s\n' "$session_lines" | grep -c . || true)
shown=$(printf '%s\n' "$session_lines" | head -20)
[ "$file_count" -gt 20 ] && shown="${shown}
... (+$((file_count - 20)) more)"

# Signature = session file list + tracked-content diff, so both a new
# file and more edits to a listed file count as "new work".
sig=$( { printf '%s\n' "$session_lines"; git -C "$CWD" diff HEAD 2>/dev/null; } \
  | shasum -a 256 | cut -d' ' -f1)
sig_changed claude-commit-nudge "$sig" || exit 0

fire "$(printf 'Stop-hook checkpoint (silent — output NO prose either way): this session left %s uncommitted file(s):\n%s\n\nIf they form one or more coherent, uncontroversial commits, commit now with scoped `git add <paths>` (NEVER `-A`; sibling sessions may have unrelated edits) and a clear message — the commit your ONLY output. If mid-flight, just stop. Never narrate this checkpoint. Quiet until the work changes.' \
  "$file_count" "$shown")"
