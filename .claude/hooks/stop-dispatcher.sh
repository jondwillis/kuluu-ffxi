#!/usr/bin/env bash
# Stop dispatcher: one hook that runs the three stop-time self-review
# checks in PRIORITY order and emits at most ONE `decision: block` per
# stop cycle.
#
# Why a single hook instead of three wired in parallel: when several
# Stop hooks each return `block`, they compete — the loud ones (commit,
# comment) crowd out the quiet one (ask-question), and the loser's
# signature never gets a clean pass against the turn that triggered it.
# Funnelling through one dispatcher makes the precedence explicit and
# guarantees the highest-priority nudge lands first; the rest surface on
# subsequent cycles once it's resolved.
#
# Priority (highest first):
#   1. ask-question — a pending decision is the only check that actually
#      blocks the *user*; surface it before housekeeping.
#   2. commit       — uncommitted session work.
#   3. comments     — comment-rot heuristics on added .rs lines.
#
# Each check keeps its own change-based signature guard (same TMPDIR
# state files as the original split hooks, so in-flight sessions carry
# over). Only the check that WINS the cycle writes its signature — the
# others stay un-updated and get their turn on a later stop.

set -uo pipefail

dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=comment-rot.lib.sh
. "$dir/comment-rot.lib.sh"

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty')
transcript=$(printf '%s' "$payload" | jq -r '.transcript_path // empty')
[ -z "$cwd" ] && cwd="$PWD"
[ -z "$session_id" ] && exit 0

# Loop guard: never block twice in one stop continuation — give the
# agent exactly one shot per stop, then let it stop cleanly.
stop_active=$(printf '%s' "$payload" | jq -r '.stop_hook_active // false')
[ "$stop_active" = "true" ] && exit 0

# emit: print the block JSON and exit. Called by the first check that fires.
emit() { jq -n --arg r "$1" '{ decision: "block", reason: $r }'; exit 0; }

# sig_changed: true (0) if $2 differs from the stored signature for
# check $1; also writes the new signature. Side-effecting on purpose —
# only ever called for the check we're about to surface.
sig_changed() { # $1=snap-subdir  $2=signature
  local snap_dir="${TMPDIR:-/tmp}/$1" sig_file
  mkdir -p "$snap_dir"
  sig_file="$snap_dir/${session_id}.sig"
  [ -f "$sig_file" ] && [ "$(cat "$sig_file")" = "$2" ] && return 1
  printf '%s' "$2" > "$sig_file"
  return 0
}

# ── 1. ask-question ─────────────────────────────────────────────
# Fire when the final assistant turn is text-only (no AskUserQuestion
# already) and the trailing prose reads as a question / set of choices.
if [ -f "$transcript" ]; then
  sel='. as $all
    | ([ range(0; length)
         | select($all[.].type == "user"
                  and ($all[.].message.content | type) == "array"
                  and ([$all[.].message.content[].type] | index("text")) != null) ]
       | last) as $u
    | [ $all[ (($u // -1) + 1) : ][] | select(.type == "assistant") | .message.content[]? ]'

  q_tools=$(jq -s -r "$sel | [ .[] | select(.type==\"tool_use\") | .name ] | unique | join(\",\")" \
    "$transcript" 2>/dev/null || true)
  q_text=$(jq -s -r "$sel | [ .[] | select(.type==\"text\") | .text ] | join(\"\n\")" \
    "$transcript" 2>/dev/null || true)

  if ! printf '%s' "$q_tools" | grep -q 'AskUserQuestion' \
     && [ -n "${q_text//[[:space:]]/}" ]; then
    last_line=$(printf '%s\n' "$q_text" | grep -v '^[[:space:]]*$' | tail -1)
    trailing_q=0
    printf '%s' "$last_line" | grep -qE '\?[[:space:]]*$' && trailing_q=1
    choice_re='(would you (like|prefer)|do you want|which (one|option|approach|of)|should i (use|go with|pick|choose)|let me know (which|if you|whether)|option [ab12]|^[[:space:]]*[abAB1-4][\).][[:space:]]|or (would|should|do) you|either .* or )'
    choice_hit=0
    printf '%s' "$q_text" | grep -qiE "$choice_re" && choice_hit=1

    if { [ "$trailing_q" -eq 1 ] || [ "$choice_hit" -eq 1 ]; } \
       && sig_changed claude-ask-question-nudge \
            "$(printf '%s' "$q_text" | shasum -a 256 | cut -d' ' -f1)"; then
      emit 'Stop-hook checkpoint (silent — output NO prose either way): your turn ended by asking the user something. If you are waiting on a real decision, re-pose it via AskUserQuestion with concrete, mutually-exclusive options (recommended first); use the elicitation tool for free-form input. Make the tool call your ONLY output. If the trailing "?" was rhetorical, an aside, or already answered, just stop. Never narrate this checkpoint. Quiet until the question text changes.'
    fi
  fi
fi

# ── 2. commit ───────────────────────────────────────────────────
# Lines dirty now but absent from the SessionStart baseline = work this
# session produced. Baseline written by commit-nudge-start.sh.
if git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1; then
  snap_file="${TMPDIR:-/tmp}/claude-commit-nudge/${session_id}.porcelain"
  current=$(git -C "$cwd" status --porcelain 2>/dev/null || true)
  if [ -f "$snap_file" ] && [ -n "$current" ]; then
    session_lines=$(comm -23 \
      <(printf '%s\n' "$current" | sort -u) \
      <(printf '%s\n' "$(cat "$snap_file")" | sort -u) \
      | grep -v '^$' || true)
    if [ -n "$session_lines" ]; then
      file_count=$(printf '%s\n' "$session_lines" | grep -c . || true)
      shown=$(printf '%s\n' "$session_lines" | head -20)
      [ "$file_count" -gt 20 ] && shown="${shown}
... (+$((file_count - 20)) more)"
      sig=$( { printf '%s\n' "$session_lines"; git -C "$cwd" diff HEAD 2>/dev/null; } \
        | shasum -a 256 | cut -d' ' -f1)
      if sig_changed claude-commit-nudge "$sig"; then
        emit "$(printf 'Stop-hook checkpoint (silent — output NO prose either way): this session left %s uncommitted file(s):\n%s\n\nIf they form one or more coherent, uncontroversial commits, commit now with scoped `git add <paths>` (NEVER `-A`; sibling sessions may have unrelated edits) and a clear message — the commit your ONLY output. If mid-flight, just stop. Never narrate this checkpoint. Quiet until the work changes.' \
          "$file_count" "$shown")"
      fi
    fi
  fi
fi

# ── 3. comments ─────────────────────────────────────────────────
# Comment-rot heuristics over the added (.rs) lines in `git diff HEAD`.
if git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1; then
  added=$(git -C "$cwd" diff HEAD -- '*.rs' 2>/dev/null \
    | grep -E '^\+' | grep -vE '^\+\+\+' | sed -E 's/^\+//' || true)
  if [ -n "$added" ]; then
    findings=$(printf '%s\n' "$added" | scan_comment_rot || true)
    if [ -n "$findings" ] \
       && sig_changed claude-comment-review \
            "$(printf '%s' "$findings" | shasum -a 256 | cut -d' ' -f1)"; then
      emit "$(printf 'Stop-hook checkpoint (silent — output NO prose either way): your uncommitted .rs changes add comment(s) matching rot heuristics:\n%s\n\nKeep one only if it explains WHY (a non-obvious rationale, constraint, or external/protocol fact); otherwise prune or fix the WHAT-restatement, narrative, decoration, or dead code. For an always/never claim, encode it (debug_assert!/newtype/enum) or cite vendor src, else soften/remove. Edit the file(s) as your ONLY output. If every flag is a false positive, just stop. Never narrate this checkpoint. Quiet until the flagged set changes.' \
        "$findings")"
    fi
  fi
fi

exit 0
