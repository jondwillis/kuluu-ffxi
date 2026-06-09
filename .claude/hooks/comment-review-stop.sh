#!/usr/bin/env bash
# Stop hook: review the comments this working tree has *added* (the
# uncommitted side of `git diff HEAD` on .rs files) against the rot
# heuristics, and — if any match — block-and-hand-back so the agent
# itself judges keep-vs-prune. This is the "LLM judge" tier without a
# second model: the agent is the judge, exactly like commit-nudge-stop
# and ask-question-nudge-stop.
#
# Silent-act contract and a change-based signature guard keep it quiet:
# it re-asks only when the flagged comment set changes.

set -uo pipefail

dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=comment-rot.lib.sh
. "$dir/comment-rot.lib.sh"

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty')
[ -z "$cwd" ] && cwd="$PWD"
[ -z "$session_id" ] && exit 0

# Loop guard: never block twice in one stop continuation.
stop_active=$(printf '%s' "$payload" | jq -r '.stop_hook_active // false')
[ "$stop_active" = "true" ] && exit 0

git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1 || exit 0

# Added lines only (drop the +++ header, strip the leading +); scan
# just the new/modified comments, never already-committed ones.
added=$(git -C "$cwd" diff HEAD -- '*.rs' 2>/dev/null \
  | grep -E '^\+' | grep -vE '^\+\+\+' | sed -E 's/^\+//' || true)
[ -z "$added" ] && exit 0

findings=$(printf '%s\n' "$added" | scan_comment_rot) || exit 0

# ── Change-based signature guard ────────────────────────────────
snap_dir="${TMPDIR:-/tmp}/claude-comment-review"
mkdir -p "$snap_dir"
sig=$(printf '%s' "$findings" | shasum -a 256 | cut -d' ' -f1)
sig_file="$snap_dir/${session_id}.sig"
if [ -f "$sig_file" ] && [ "$(cat "$sig_file")" = "$sig" ]; then
  exit 0
fi
printf '%s' "$sig" > "$sig_file"

reason=$(printf 'Stop-hook checkpoint (act silently — output NO prose in response to this checkpoint either way): your uncommitted .rs changes add comment(s) matching rot heuristics:\n%s\n\nReview each against the comment policy: keep it only if it explains WHY (a non-obvious rationale, constraint, or external/protocol fact). Delete or fix WHAT-restatement, session-history/narrative, decoration, and commented-out code; for an "always/never" claim, either encode it (debug_assert!/newtype/enum) or cite vendor src, otherwise soften/remove it; replace a restated literal/offset with a named const or drop the duplication. Edit the file(s) to prune/repair, making the edits your ONLY output — no narration. If every flag here is a false positive, just stop with no text. This checkpoint stays quiet until the flagged set changes.' \
  "$findings")

jq -n --arg r "$reason" '{ decision: "block", reason: $r }'
