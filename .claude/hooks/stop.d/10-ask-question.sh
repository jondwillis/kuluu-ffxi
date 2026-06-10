#!/usr/bin/env bash
# Stop sub-check (priority 10): if the final assistant turn is text-only
# (no AskUserQuestion already) and the trailing prose reads as a question
# or set of choices, fire — nudging the agent to re-pose it via
# AskUserQuestion. A pending decision is the only check that actually
# blocks the user, so it runs first.
#
# Contract: see stop-lib.sh. Exit 0 = pass; fire = exit 10 + reason.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=../stop-lib.sh
. "$here/../stop-lib.sh"
load_payload

[ -f "$TRANSCRIPT" ] || exit 0

# Wait for the final assistant text block to land, then pull everything we
# need in ONE slurp. The Stop hook can read $TRANSCRIPT a beat before the
# model's closing prose is durably appended (observed: a question written
# at T+0 wasn't on disk when hooks ran ~0.3s later). A turn that ends
# naturally ends in a text block; while the last assistant entry still ends
# in thinking/tool_use the tail is mid-flush, so re-read up to ~0.5s.
# Judging a stale tail would miss the trailing "?" and pass silently — and
# on a clean tree (nothing for the lower checks to fire on, so no
# continuation) there's no second chance. One slurp per attempt, one total
# once settled.
#
# $blocks = assistant content after the last turn boundary: the most recent
# user message that ISN'T a pure tool_result carrier. That boundary set is
# (a) genuine prompts — string content OR an array carrying a text block —
# and (b) our own injected Stop-hook checkpoints, which land as string-content
# user messages (isMeta). Resetting at the checkpoint is the whole point: it
# scopes the judged text to JUST the latest continuation turn.
#
# An earlier version reset only at array+text prompts. Checkpoints (strings)
# then weren't boundaries, so $blocks accumulated EVERY assistant text block
# since the real prompt and grew each cycle. Two failures compounded into a
# loop: (1) the signature is a hash of that text, so growing text => new sig
# every cycle => sig_changed never suppressed a re-fire; (2) choice_re is
# matched over the whole blob, so one stale "1." / "2." list or a long-gone
# question latched choice_hit=1 forever. Net: re-fired every turn until the
# dispatcher's depth backstop. Scoping to the last turn kills both. ready
# keys off the last assistant entry in the whole transcript.
extract='. as $all
  | ([ range(0; length)
       | select($all[.].type == "user"
                and ( ($all[.].message.content | type) == "string"
                      or ([$all[.].message.content[]?.type] | index("text")) != null )) ]
     | last) as $u
  | ([ $all[ (($u // -1) + 1) : ][] | select(.type == "assistant") | .message.content[]? ]) as $blocks
  | (([ $all[] | select(.type == "assistant") ] | last) // {}) as $lastA
  | { ready: (($lastA.message.content[-1].type?) == "text"),
      tools: ([ $blocks[] | select(.type == "tool_use") | .name ] | unique),
      text:  ([ $blocks[] | select(.type == "text") | .text ] | join("\n")) }'

result='{}'
for _ in 1 2 3 4 5; do
  result=$(jq -s -c "$extract" "$TRANSCRIPT" 2>/dev/null || echo '{}')
  [ "$(printf '%s' "$result" | jq -r '.ready // false')" = "true" ] && break
  sleep 0.1
done

tools=$(printf '%s' "$result" | jq -r '.tools | join(",")' 2>/dev/null || true)
text=$(printf '%s' "$result" | jq -r '.text // ""' 2>/dev/null || true)

# Already re-posed via the tool this turn, or no prose at all — pass.
# (grep, not ${text//[[:space:]]/}: that bash substitution is ~O(n^2) under
# macOS bash 3.2 and burned ~8s on a few KB of prose.)
printf '%s' "$tools" | grep -q 'AskUserQuestion' && exit 0
printf '%s' "$text" | grep -q '[^[:space:]]' || exit 0

# Strong signal: the last non-empty line ends with "?".
last_line=$(printf '%s\n' "$text" | grep -v '^[[:space:]]*$' | tail -1)
trailing_q=0
printf '%s' "$last_line" | grep -qE '\?[[:space:]]*$' && trailing_q=1

# Choice/elicitation phrasing anywhere in the turn (case-insensitive).
choice_re='(would you (like|prefer)|do you want|which (one|option|approach|of)|should i (use|go with|pick|choose)|let me know (which|if you|whether)|option [ab12]|^[[:space:]]*[abAB1-4][\).][[:space:]]|or (would|should|do) you|either .* or )'
choice_hit=0
printf '%s' "$text" | grep -qiE "$choice_re" && choice_hit=1

{ [ "$trailing_q" -eq 1 ] || [ "$choice_hit" -eq 1 ]; } || exit 0

sig_changed claude-ask-question-nudge \
  "$(printf '%s' "$text" | shasum -a 256 | cut -d' ' -f1)" || exit 0

fire 'Stop-hook checkpoint (silent — output NO prose either way): your turn ended by asking the user something. If you are waiting on a real decision, re-pose it via AskUserQuestion with concrete, mutually-exclusive options (recommended first); use the elicitation tool for free-form input. Make the tool call your ONLY output. If the trailing "?" was rhetorical, an aside, or already answered, just stop. Never narrate this checkpoint. Quiet until the question text changes.'
