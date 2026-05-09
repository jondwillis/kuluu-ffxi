#!/bin/bash
# SessionStart hook for the FFXI agent: inject the persisted goal and
# last-event sidecar contents into the model's context so the bootstrap
# turn doesn't have to spend 4 round-trips re-deriving "where am I,
# what was I doing, what just happened".
#
# Output goes back to Claude Code as JSON with hookSpecificOutput.
# additionalContext, which the harness folds into the system prompt for
# the first turn only.

set -euo pipefail

CONFIG_DIR="${FFXI_MCP_CONFIG_DIR:-$HOME/.config/ffxi-mcp}"
GOAL_FILE="${FFXI_MCP_GOAL_PATH:-$CONFIG_DIR/goal.json}"
EVENT_FILE="${FFXI_MCP_EVENT_PATH:-$CONFIG_DIR/last-event.json}"

goal_blob='{"goal":"idle"}'
if [ -r "$GOAL_FILE" ]; then
  raw=$(cat "$GOAL_FILE" 2>/dev/null || echo "")
  if [ -n "$raw" ] && printf '%s' "$raw" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    goal_blob="$raw"
  fi
fi

event_blob='null'
event_age_note=""
if [ -r "$EVENT_FILE" ]; then
  raw=$(cat "$EVENT_FILE" 2>/dev/null || echo "")
  if [ -n "$raw" ] && printf '%s' "$raw" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    event_blob="$raw"
    now_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
    event_at=$(printf '%s' "$raw" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("at_unix_ms",0))')
    age_ms=$(( now_ms - event_at ))
    event_age_note=" (${age_ms}ms ago)"
  fi
fi

context=$(python3 - <<PY
import json, os
goal = json.loads('''$goal_blob''')
event = json.loads('''$event_blob''')
note = "$event_age_note"
lines = []
lines.append("FFXI agent bootstrap context (from sidecars; saves a turn of MCP round-trips):")
lines.append("")
if goal.get("goal") == "idle" or not goal:
    lines.append("- Persisted goal: idle. Per the autonomy contract you must pick a non-Idle goal this turn.")
else:
    lines.append(f"- Persisted goal: {json.dumps(goal)} — the supervisor is already running this; do NOT re-issue. Just react to events.")
if event:
    lines.append(f"- Last high-signal event{note}: {json.dumps(event)}")
    lines.append("  If recent (<30s), this is likely the reason you woke up — react before exploring.")
else:
    lines.append("- No recent high-signal event sidecar. Cold start or fresh session.")
lines.append("")
lines.append("Still call snapshot once and read scene://entities for current ground truth — sidecars are point-in-time, not live.")
print("\\n".join(lines))
PY
)

python3 - <<PY
import json, sys
ctx = """$context"""
print(json.dumps({
    "hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "additionalContext": ctx,
    }
}))
PY
