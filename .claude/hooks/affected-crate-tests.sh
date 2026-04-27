#!/usr/bin/env bash
# PostToolUse hook: after an Edit/Write to a workspace crate's
# source file, run that crate's lib tests. Cheap when the crate
# is small (typically <2s); catches assertion regressions before
# they ride to the next conversation turn.
#
# Scope: only runs --lib tests of one crate, not the whole
# workspace. Integration tests and full builds are too slow for
# a per-edit hook.
#
# Exits 0 unconditionally so test failures surface as stderr
# context for Claude rather than blocking the tool call. The
# point is fast feedback, not enforcement.

set -uo pipefail

payload=$(cat)
file=$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(d.get("tool_input",{}).get("file_path",""), end="")' \
  2>/dev/null || true)

if [ -z "$file" ]; then
  exit 0
fi

# Extract workspace-relative crate name. Looks for the first
# segment after `ffxi/` matching `ffxi-*` (or any workspace
# member). If the edit isn't under a crate src/, skip.
crate=$(printf '%s' "$file" | sed -nE 's|.*/(ffxi-[^/]+)/(src|tests)/.*|\1|p')

if [ -z "$crate" ]; then
  exit 0
fi

# Run the tests in the background with a short timeout. If they
# pass, the hook is silent. If they fail, the tail goes to stderr
# so Claude sees the failure on its next turn.
output=$(cargo test -p "$crate" --lib --quiet 2>&1 | tail -10 || true)
fail_count=$(printf '%s' "$output" | grep -cE 'FAILED|test result: FAILED' || true)

if [ "${fail_count:-0}" -gt 0 ]; then
  cat >&2 <<MSG
[affected-crate-tests] $crate lib tests failed after edit to $file:
$output
MSG
fi

exit 0
