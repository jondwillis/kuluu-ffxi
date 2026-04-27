#!/bin/bash
# Stop hook for the FFXI agent: keep the loop alive, but only when there's
# actually something to do.
#
# Reads two sidecar files written by ffxi-mcp:
#   ~/.config/ffxi-mcp/goal.json         — current reactor goal (idle/active)
#   ~/.config/ffxi-mcp/last-event.json   — most recent high-signal event
#
# Decision matrix:
#   - FFXI_AUTONOMY_OFF set                 → exit 0 (operator opted out)
#   - stop_hook_active=true (loop guard)    → exit 0 (avoid infinite block)
#   - goal active AND no fresh event        → exit 0 (reactor is working)
#   - fresh event in last 30s               → block + inject event payload
#   - goal idle AND no fresh event          → block (generic "pick a goal")
#
# "Fresh" = within RECENT_EVENT_WINDOW_MS. The reactor's auto-mechanics
# (follow, engage, path_to) keep working between LLM turns, so a long-
# running goal doesn't need the LLM woken every 200ms; we only block on
# real signal.

set -euo pipefail

if [ -n "${FFXI_AUTONOMY_OFF:-}" ]; then
  exit 0
fi

input=$(cat)
if printf '%s' "$input" | grep -q '"stop_hook_active"[[:space:]]*:[[:space:]]*true'; then
  exit 0
fi

CONFIG_DIR="${FFXI_MCP_CONFIG_DIR:-$HOME/.config/ffxi-mcp}"
GOAL_FILE="${FFXI_MCP_GOAL_PATH:-$CONFIG_DIR/goal.json}"
EVENT_FILE="${FFXI_MCP_EVENT_PATH:-$CONFIG_DIR/last-event.json}"
RECENT_EVENT_WINDOW_MS=30000

block() {
  # $1 = reason text
  reason=$(printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
  printf '{"decision":"block","reason":%s}\n' "$reason"
  exit 0
}

# ---- Inspect last-event sidecar -------------------------------------
event_kind=""
event_payload=""
event_age_ms=""
if [ -r "$EVENT_FILE" ]; then
  now_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
  event_kind=$(python3 -c "import json,sys; d=json.load(open('$EVENT_FILE')); print(d.get('kind',''))" 2>/dev/null || echo "")
  event_at=$(python3 -c "import json,sys; d=json.load(open('$EVENT_FILE')); print(d.get('at_unix_ms',0))" 2>/dev/null || echo "0")
  event_payload=$(python3 -c "import json,sys; d=json.load(open('$EVENT_FILE')); print(json.dumps(d.get('payload',{})))" 2>/dev/null || echo "{}")
  event_age_ms=$(( now_ms - event_at ))
fi

# ---- Inspect goal sidecar -------------------------------------------
goal_state="idle"
goal_blob="{}"
if [ -r "$GOAL_FILE" ]; then
  # Goal file from GoalStore::save serializes the AgentCommand directly
  # when active; missing/empty file = idle.
  goal_blob=$(cat "$GOAL_FILE" 2>/dev/null || echo "{}")
  if printf '%s' "$goal_blob" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d else 1)' 2>/dev/null; then
    goal_state="active"
  fi
fi

# ---- Decide ---------------------------------------------------------
if [ -n "$event_kind" ] && [ "$event_age_ms" -lt "$RECENT_EVENT_WINDOW_MS" ]; then
  block "A high-signal event fired ${event_age_ms}ms ago: kind=${event_kind}, payload=${event_payload}. Per the autonomy contract, react to it: re-read scene://entities and party://members, then dispatch the appropriate tool (cast/engage/follow/path_to). Use wait_for_event { timeout_ms: 5000 } if you need to settle and let the next event surface before deciding."
fi

if [ "$goal_state" = "active" ]; then
  # Reactor is autonomously working a goal; let it run. The LLM doesn't
  # need a turn for every 200ms tick. Only block here if there's also no
  # event — which we already handled above.
  exit 0
fi

block "Goal is idle and no recent event. Per the autonomy contract you must pick a non-Idle goal: read scene://entities to find a target, then engage/follow/path_to. If you genuinely have nothing actionable (e.g. zone has no entities), wait_for_event { timeout_ms: 30000 } until a tell or party invite arrives. Operator can exit by relaunching with FFXI_AUTONOMY_OFF=1."
