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

# Assistant content after the last genuine user prompt (a user message
# carrying a text block — tool_result user messages don't count).
sel='. as $all
  | ([ range(0; length)
       | select($all[.].type == "user"
                and ($all[.].message.content | type) == "array"
                and ([$all[.].message.content[].type] | index("text")) != null) ]
     | last) as $u
  | [ $all[ (($u // -1) + 1) : ][] | select(.type == "assistant") | .message.content[]? ]'

tools=$(jq -s -r "$sel | [ .[] | select(.type==\"tool_use\") | .name ] | unique | join(\",\")" \
  "$TRANSCRIPT" 2>/dev/null || true)
text=$(jq -s -r "$sel | [ .[] | select(.type==\"text\") | .text ] | join(\"\n\")" \
  "$TRANSCRIPT" 2>/dev/null || true)

# Already re-posed via the tool this turn, or no prose at all — pass.
printf '%s' "$tools" | grep -q 'AskUserQuestion' && exit 0
[ -z "${text//[[:space:]]/}" ] && exit 0

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
