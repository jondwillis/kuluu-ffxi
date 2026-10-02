#!/usr/bin/env bash
# PreToolUse(Bash): snapshot the dirty set so the Post counterpart can
# attribute whatever this command writes. Covers the writes Edit/Write never
# see — sed -i, cargo fmt, rm, heredocs — as far as the command text names its
# outputs or has a writer form; anything else Post logs as a suspect instead.
#
# A sibling .sig file records a content hash per path, for the dirty set and
# for every existing file the command names. Post needs it because the
# porcelain delta alone cannot tell "this command wrote it" from "a peer wrote
# it while this command ran", and cannot see a further edit to a path that was
# already dirty.
#
# Contract: never blocks, never speaks.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=./session-edits.lib.sh
. "$here/session-edits.lib.sh"

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty' 2>/dev/null || true)
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty' 2>/dev/null || true)
[ -z "$cwd" ] && cwd="$PWD"
[ -n "$session_id" ] || exit 0
git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1 || exit 0

cmd=$(printf '%s' "$payload" | jq -r '.tool_input.command // empty' 2>/dev/null || true)

# Porcelain lines, signatures and ledger entries are all relative to the
# worktree root, never to a cwd that may be a subdirectory of it.
root=$(repo_root "$cwd")

mkdir -p "$(snap_dir)" || exit 0
snap_sweep
ledger_touch "$session_id"
ledger_sweep
snap=$(snap_path "$session_id" "$cmd") || exit 0
git -C "$root" status --porcelain 2>/dev/null > "$snap" || true

rm -f "$snap.sig"
{
  porcelain_paths < "$snap" 2>/dev/null || true
  cmd_path_tokens "$cmd" "$cwd" "$root"
} | sig_snapshot "$root" "$snap.sig"
exit 0
