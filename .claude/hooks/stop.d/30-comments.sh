#!/usr/bin/env bash
# Stop sub-check (priority 30): run the comment-rot heuristics over the
# comments THIS SESSION added to .rs files. If any match, fire so the agent
# judges keep-vs-prune — the "LLM judge" tier with the agent as judge.
#
# Scope: only files this session itself dirtied, derived from the
# SessionStart porcelain baseline (the same one 20-commit uses). Without
# this, the check nagged about inherited dirty .rs files the agent never
# touched — and refused to rewrite another session's in-flight comments
# every cycle, which is just noise.
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

# Files this session dirtied = porcelain lines present now but absent from
# the SessionStart baseline (written by commit-nudge-start.sh). No baseline
# → can't separate session work from inherited dirt, so stay quiet.
snap_file="${TMPDIR:-/tmp}/claude-commit-nudge/${SESSION_ID}.porcelain"
[ -f "$snap_file" ] || exit 0

current=$(git -C "$CWD" status --porcelain 2>/dev/null || true)
[ -z "$current" ] && exit 0

# comm -23 needs sorted inputs. Strip the XY status prefix to bare paths,
# resolve renames to their new path (`R old -> new`), keep only .rs.
session_rs=$(comm -23 \
  <(printf '%s\n' "$current" | sort -u) \
  <(printf '%s\n' "$(cat "$snap_file")" | sort -u) \
  | grep -v '^$' \
  | sed -E 's/^...//; s/^.* -> //' \
  | grep -E '\.rs$' || true)
[ -z "$session_rs" ] && exit 0

# Added lines only (drop the +++ header, strip the leading +); scan just
# the new/modified comments in session-touched files, never committed ones.
# (git diff HEAD shows nothing for untracked files — same as the old '*.rs'
# pathspec, so brand-new files stay out of scope here, as before.)
added=$(printf '%s\n' "$session_rs" \
  | xargs git -C "$CWD" diff HEAD -- 2>/dev/null \
  | grep -E '^\+' | grep -vE '^\+\+\+' | sed -E 's/^\+//' || true)
[ -z "$added" ] && exit 0

findings=$( { printf '%s\n' "$added" | scan_comment_rot || true; \
              printf '%s\n' "$added" | scan_code_magic || true; } \
            | grep -v '^[[:space:]]*$' || true)
[ -z "$findings" ] && exit 0

sig_changed claude-comment-review \
  "$(printf '%s' "$findings" | shasum -a 256 | cut -d' ' -f1)" || exit 0

rmcm_cmd=$(printf '%s' "$session_rs" | tr '\n' ' ')
fire "$(printf 'Stop-hook checkpoint (silent — output NO prose either way): this project bans narrative code comments, and your uncommitted .rs changes add:\n%s\n\nDelete each one unless it is a WHY you cannot encode (then keep it tight), a citation to vendor/protocol/spec source, or a SAFETY justification — for a magic literal name it as a const; for an always/never claim, encode it (debug_assert!/newtype/enum) or cite vendor src instead. Doc comments (/// //!) are held to the same bar — tighten or prune rambling/stale ones. Edit the file(s) as your ONLY output. (To bulk-strip, `rmcm --diff -l rust %s` then apply selectively — rmcm removes ALL comments, including the doc/SAFETY/citations you must keep.) If every flag is a false positive, just stop. Never narrate this checkpoint. Quiet until the flagged set changes.' \
  "$findings" "$rmcm_cmd")"
