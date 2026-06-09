#!/usr/bin/env bash
# PreToolUse hook (Edit|Write): scan the comment lines in the text
# *about to be written* for rot heuristics and surface a one-line,
# non-blocking heads-up so the agent self-corrects before the edit
# lands. Mirrors lsb-boundary-reminder.sh — stderr + exit 0, never
# blocks (the regexes are too blunt to gate work on).
#
# For Edit the incoming text is tool_input.new_string; for Write it is
# tool_input.content. Either way we only see what is being added, which
# is exactly the new comments — pre-existing ones are not re-flagged.

set -uo pipefail

dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=comment-rot.lib.sh
. "$dir/comment-rot.lib.sh"

payload=$(cat)
text=$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys; d=json.load(sys.stdin).get("tool_input",{}); print(d.get("new_string", d.get("content","")), end="")' \
  2>/dev/null || true)

[ -z "$text" ] && exit 0
case "$text" in *//*) ;; *) exit 0 ;; esac  # no line comments → nothing to do

findings=$(printf '%s\n' "$text" | scan_comment_rot) || exit 0

cat >&2 <<MSG
[comment-rot] The text you're about to write has comment(s) matching rot heuristics:
$findings
Prefer self-documenting code: comments carry WHY, names carry WHAT/HOW. Drop narrative/history, decoration, and commented-out code; for "always/never" encode the invariant (assert/newtype/enum) or cite vendor src; don't restate adjacent literals. Ignore if a flag is a false positive.
MSG
exit 0
