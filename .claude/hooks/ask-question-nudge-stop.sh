#!/usr/bin/env bash
# Stop hook: when the agent ends a turn by posing a question or a set
# of choices to the user in *prose*, nudge it to re-pose them with the
# AskUserQuestion / elicitation tool instead — structured options the
# user can pick beat a wall of "would you like A or B?" prose.
#
# Mirrors commit-nudge-stop.sh: loop guard, change-based signature
# guard so an already-declined message stays quiet, and a `decision:
# block` hand-back so the agent wakes up and re-asks via the tool.
#
# The check is intentionally conservative — it only fires when the
# final assistant turn is text-only (no AskUserQuestion already) AND
# the trailing prose reads as a question/choice addressed to the user.

set -euo pipefail

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
transcript=$(printf '%s' "$payload" | jq -r '.transcript_path // empty')
[ -z "$session_id" ] && exit 0
[ -f "$transcript" ] || exit 0

# Loop guard: never block twice in one stop continuation.
stop_active=$(printf '%s' "$payload" | jq -r '.stop_hook_active // false')
[ "$stop_active" = "true" ] && exit 0

# ── Extract the final assistant turn ────────────────────────────
# Everything after the last genuine user prompt (a user message that
# carries a text block — tool_result user messages don't count). Two
# passes (NUL can't survive command substitution): the tool names
# used, then the concatenated assistant text. Shared selector in $sel.
sel='. as $all
  | ([ range(0; length)
       | select($all[.].type == "user"
                and ($all[.].message.content | type) == "array"
                and ([$all[.].message.content[].type] | index("text")) != null) ]
     | last) as $u
  | [ $all[ (($u // -1) + 1) : ][] | select(.type == "assistant") | .message.content[]? ]'

tools=$(jq -s -r "$sel | [ .[] | select(.type==\"tool_use\") | .name ] | unique | join(\",\")" \
  "$transcript" 2>/dev/null || true)
text=$(jq -s -r "$sel | [ .[] | select(.type==\"text\") | .text ] | join(\"\n\")" \
  "$transcript" 2>/dev/null || true)

# Already did the right thing this turn — stay quiet.
printf '%s' "$tools" | grep -q 'AskUserQuestion' && exit 0
[ -z "${text//[[:space:]]/}" ] && exit 0

# ── Does the trailing prose pose a question / set of choices? ────
# Strong signal: the last non-empty line ends with "?".
last_line=$(printf '%s\n' "$text" | grep -v '^[[:space:]]*$' | tail -1)
trailing_q=0
printf '%s' "$last_line" | grep -qE '\?[[:space:]]*$' && trailing_q=1

# Choice/elicitation phrasing anywhere in the turn (case-insensitive).
choice_re='(would you (like|prefer)|do you want|which (one|option|approach|of)|should i (use|go with|pick|choose)|let me know (which|if you|whether)|option [ab12]|^[[:space:]]*[abAB1-4][\).][[:space:]]|or (would|should|do) you|either .* or )'
choice_hit=0
printf '%s' "$text" | grep -qiE "$choice_re" && choice_hit=1

# ─── TUNE-ME ───────────────────────────────────────────────────
# Require a real prompt-to-the-user signal. Default: fire when the
# turn ends on a question mark, OR uses explicit choice phrasing.
# Tighten by requiring BOTH ( [ "$trailing_q" = 1 ] && ... ), or add
# a floor on question count, etc.
# ───────────────────────────────────────────────────────────────
if [ "$trailing_q" -eq 0 ] && [ "$choice_hit" -eq 0 ]; then exit 0; fi

# ── Change-based signature guard ────────────────────────────────
# Re-ask only when the question text itself changes. An already-
# declined prose question stays quiet until the agent phrases a
# different one.
snap_dir="${TMPDIR:-/tmp}/claude-ask-question-nudge"
mkdir -p "$snap_dir"
sig=$(printf '%s' "$text" | shasum -a 256 | cut -d' ' -f1)
sig_file="$snap_dir/${session_id}.sig"
if [ -f "$sig_file" ] && [ "$(cat "$sig_file")" = "$sig" ]; then
  exit 0
fi
printf '%s' "$sig" > "$sig_file"

# Block the stop and hand the decision back. The agent wakes up, judges
# whether the prose genuinely asks the user to choose, and if so re-poses
# it through AskUserQuestion (concrete, mutually-exclusive options) — or
# the elicitation tool for free-form input — rather than prose.
reason='Stop-hook checkpoint (act silently — output NO prose in response to this checkpoint either way): this turn ends by asking the user something in prose. If you are genuinely waiting on a decision or choice, re-pose it by calling the AskUserQuestion tool immediately — gather the question(s) and offer concrete, mutually-exclusive options (mark a recommended one first) so the user can pick rather than type; use the elicitation tool when you need free-form input. Make the tool call your ONLY output — do not preface or explain it. If the trailing "?" was rhetorical, an aside, or already answered, just stop with no further text. Never narrate this checkpoint or analyze whether it was a false positive. It stays quiet until the question text changes.'

jq -n --arg r "$reason" '{ decision: "block", reason: $r }'
