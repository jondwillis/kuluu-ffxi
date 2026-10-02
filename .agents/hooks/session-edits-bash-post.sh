#!/usr/bin/env bash
# PostToolUse(Bash): work out which paths THIS command wrote and append them
# to the session edit ledger.
#
# The porcelain delta alone is not that answer: in a shared checkout a peer
# session's concurrent write lands in the same delta, and a whole-line diff
# even turns a peer's `git add` into a phantom entry, because the status
# columns change while the file does not. So a path is credited only when its
# content signature actually moved AND the command either names it or has a
# writer form. Content changed but neither — almost always the peer — goes to
# the suspect log instead, where a human can read it.
#
# A stale snapshot would mis-attribute a neighbour's concurrent write, so
# snapshots older than SESSION_EDITS_SNAP_TTL seconds are discarded rather
# than trusted.
#
# Attribution is quiet; runtime-ledger failures propagate to the adapter.

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
root=$(repo_root "$cwd")
prefix=$(subdir_prefix "$root" "$cwd")
snap=$(snap_path "$session_id" "$cmd") || exit 0
sigs="$snap.sig"
[ -f "$snap" ] || { rm -f "$sigs"; exit 0; }

snap_mtime=$(file_mtime "$snap")
now=$(date +%s)
if [ "$((now - snap_mtime))" -gt "$SESSION_EDITS_SNAP_TTL" ]; then
  rm -f "$snap" "$sigs"
  exit 0
fi

# Paths dirty now that were not dirty before, plus every path the pre-hook
# signed — the second arm is what sees a further edit to an already-dirty file,
# whose porcelain line never changes.
delta=$(comm -13 \
  <(sort -u "$snap") \
  <(git -C "$root" status --porcelain 2>/dev/null | sort -u) \
  | porcelain_paths || true)
signed=""
[ -f "$sigs" ] && signed=$(cut -f2- "$sigs")
candidates=$(printf '%s\n%s\n' "$delta" "$signed" | grep -v '^$' | sort -u || true)
rm -f "$snap"

post_sigs="$sigs.post"
if [ -n "$candidates" ]; then
  printf '%s\n' "$candidates" | sig_snapshot "$root" "$post_sigs"
  # post_sigs covers every candidate, so a signed line with no identical
  # counterpart before the command is exactly a candidate whose content moved.
  capped=0
  if [ -f "$post_sigs" ]; then
    changed=$(comm -13 \
      <(sort -u "$sigs" 2>/dev/null) \
      <(sort -u "$post_sigs") | cut -f2- || true)
  else
    # Over the signature cap there is no evidence any particular path moved, so
    # the pre-command dirty set is dropped entirely (crediting it is the very
    # misattribution this hook exists to stop) and only the command's own named
    # paths, out of what newly went dirty, can be credited.
    capped=1
    changed="$delta"
  fi
  writer=0
  [ "$capped" = 0 ] && cmd_is_writer_form "$cmd" && writer=1
  reason="$SESSION_EDITS_SUSPECT_UNNAMED"
  [ "$capped" = 1 ] && reason="$SESSION_EDITS_SUSPECT_CAPPED"
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    if { [ "$writer" = 1 ] && cmd_writer_plausible "$cmd" "$cwd" "$root" "$p"; } \
      || cmd_names_path "$cmd" "$root" "$prefix" "$p" \
      || { [ "$capped" = 0 ] && cmd_owns_path "$cmd" "$p"; }; then
      ledger_add "$session_id" "$root" "$p" || exit 1
    else
      suspect_add "$session_id" "$root" "$p" "$reason"
    fi
  done <<< "$changed"
fi
rm -f "$sigs" "$post_sigs"
ledger_touch "$session_id"
exit 0
