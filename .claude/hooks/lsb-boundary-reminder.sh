#!/usr/bin/env bash
# PreToolUse hook: when Claude is about to Edit/Write a file that
# already cites `vendor/server/` or `vendor/Phoenix/` in its
# comments, surface a one-line reminder to cross-check against LSB.
#
# Self-maintaining registry: any file that needs LSB-check should
# already carry a citation comment per the `lsb-mirror-check`
# skill's convention. If a file _should_ be on the list but isn't
# yet, the citation is missing — that's its own finding.
#
# Stdin from Claude Code carries a JSON envelope with the tool
# call. We extract `tool_input.file_path` (Edit/Write convention).
# If the file doesn't exist yet, no citations to check, no
# reminder.

set -euo pipefail

payload=$(cat)
file=$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(d.get("tool_input",{}).get("file_path",""), end="")' \
  2>/dev/null || true)

if [ -z "$file" ] || [ ! -f "$file" ]; then
  exit 0
fi

# Only check text files we can grep. Skip large binaries.
if [ "$(wc -c < "$file" 2>/dev/null || echo 0)" -gt 1048576 ]; then
  exit 0
fi

if grep -qE 'vendor/(server|Phoenix)/' "$file" 2>/dev/null; then
  # Surface the citation context so the implementer knows where
  # to look. Only the first few citations are shown to keep the
  # reminder small.
  cat >&2 <<MSG
[lsb-boundary-reminder] '$file' cites vendor/server/ or vendor/Phoenix/ — this is LSB-boundary code.
Before merging, verify the change still matches LSB's authoritative source. Existing citations in this file:
$(grep -nE 'vendor/(server|Phoenix)/' "$file" | head -3 | sed 's/^/  /')
Use /lsb-mirror-check if you're unsure which LSB symbol to compare against.
MSG
fi

exit 0
