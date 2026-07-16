#!/usr/bin/env bash
# SessionStart baseline for the verify gate (stop.d/25-verify.sh).
#
# Records where the tree stood when the session began so the gate can
# later tell "source changed THIS session" apart from pre-existing drift:
#   <tmp>/claude-verify-gate/<session>.head       HEAD sha at start
#   <tmp>/claude-verify-gate/<session>.porcelain  dirty snapshot at start
# (Own snapshots rather than reusing claude-commit-nudge's: the two checks
# must stay independently removable.)
#
# Emits nothing — baselines only.

set -uo pipefail

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty')
[ -z "$cwd" ] && cwd="$PWD"
[ -z "$session_id" ] && exit 0

git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1 || exit 0

snap_dir="${TMPDIR:-/tmp}/claude-verify-gate"
mkdir -p "$snap_dir"

git -C "$cwd" rev-parse HEAD 2>/dev/null > "$snap_dir/${session_id}.head" || exit 0
git -C "$cwd" status --porcelain 2>/dev/null > "$snap_dir/${session_id}.porcelain" || true

exit 0
