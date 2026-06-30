#!/usr/bin/env bash
# SessionStart hook: surface the live beads backlog and position beads
# alongside this repo's existing conventions.
#
# We deliberately do NOT pipe raw `bd prime`: its CLI-mode output instructs
# agents to abandon TodoWrite/TaskCreate and MEMORY.md, which conflicts with
# this repo's auto-memory system. Instead we inject live `bd ready` state plus a
# one-line pointer to AGENTS.md for the full conventions. Always exits 0 so a
# missing/slow bd never blocks session start.

set -uo pipefail

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

payload=$(cat)
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty' 2>/dev/null)
[ -z "$cwd" ] && cwd="$PWD"

command -v bd >/dev/null 2>&1 || exit 0
[ -d "$cwd/.beads" ] || exit 0

ready=$(cd "$cwd" && bd ready 2>/dev/null | head -10)
[ -z "$ready" ] && ready="(none ready — all open issues are blocked or in progress)"

counts=$(cd "$cwd" && bd stats 2>/dev/null \
  | grep -E 'Open:|In Progress:|Ready to Work:' \
  | sed 's/^[[:space:]]*/  /')
[ -z "$counts" ] && counts="  (bd stats unavailable)"

msg=$(cat <<EOF
Beads (\`bd\`) is the single source of truth for all durable work in this repo — pick work from \`bd ready\`; the grounded parity backlog is the \`roadmap\`-labelled beads. It sits alongside, and does not replace:
- MEMORY.md auto-memory (cross-session memory) — do NOT migrate it into \`bd remember\`.
- TaskCreate, for ephemeral in-session todos.
GitHub Issues are a generated projection of beads (\`scripts/beads-github-publish.py\`), not a second tracker. Full conventions: AGENTS.md → "Issue tracking (beads)".

Issue counts:
$counts

Ready to work (\`bd ready\` for full list):
$ready
EOF
)

jq -n --arg m "$msg" '{
  hookSpecificOutput: { hookEventName: "SessionStart", additionalContext: $m }
}'
