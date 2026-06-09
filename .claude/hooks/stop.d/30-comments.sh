#!/usr/bin/env bash
# Stop sub-check (priority 30): run the comment-rot heuristics over the
# comments this working tree has *added* (the uncommitted side of
# `git diff HEAD` on .rs files). If any match, fire so the agent judges
# keep-vs-prune — the "LLM judge" tier with the agent as judge.
#
# Contract: see stop-lib.sh. Exit 0 = pass; fire = exit 10 + reason.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=../stop-lib.sh
. "$here/../stop-lib.sh"
# shellcheck source=../comment-rot.lib.sh
. "$here/../comment-rot.lib.sh"
load_payload

git -C "$CWD" rev-parse --git-dir >/dev/null 2>&1 || exit 0

# Added lines only (drop the +++ header, strip the leading +); scan just
# the new/modified comments, never already-committed ones.
added=$(git -C "$CWD" diff HEAD -- '*.rs' 2>/dev/null \
  | grep -E '^\+' | grep -vE '^\+\+\+' | sed -E 's/^\+//' || true)
[ -z "$added" ] && exit 0

findings=$(printf '%s\n' "$added" | scan_comment_rot || true)
[ -z "$findings" ] && exit 0

sig_changed claude-comment-review \
  "$(printf '%s' "$findings" | shasum -a 256 | cut -d' ' -f1)" || exit 0

fire "$(printf 'Stop-hook checkpoint (silent — output NO prose either way): your uncommitted .rs changes add comment(s) matching rot heuristics:\n%s\n\nKeep one only if it explains WHY (a non-obvious rationale, constraint, or external/protocol fact); otherwise prune or fix the WHAT-restatement, narrative, decoration, or dead code. For an always/never claim, encode it (debug_assert!/newtype/enum) or cite vendor src, else soften/remove. Edit the file(s) as your ONLY output. If every flag is a false positive, just stop. Never narrate this checkpoint. Quiet until the flagged set changes.' \
  "$findings")"
