#!/usr/bin/env bash
#
# create-issues.sh — mirror the parity scoreboard in docs/ROADMAP.md into
# GitHub issues. Run this once after the public repo exists.
#
# Source of truth is docs/ROADMAP.md: every non-done checklist item ([ ], [~],
# [?]) becomes one issue, labelled by scoreboard (vanilla-parity / enhanced),
# area, and status. Done items ([x]) are skipped. Re-runnable: an item whose
# title already exists (open or closed) is skipped, so you can re-run after
# adding rows to the roadmap.
#
# Usage:
#   scripts/create-issues.sh                       # -> jondwillis/kuluu-ffxi
#   scripts/create-issues.sh owner/repo            # -> a different repo
#   REPO=owner/repo scripts/create-issues.sh
#   DRY_RUN=1 scripts/create-issues.sh             # print, create nothing
#
# Requires: gh (authenticated, `gh auth status`), awk, bash.
set -euo pipefail

REPO="${1:-${REPO:-jondwillis/kuluu-ffxi}}"
ROADMAP="$(cd "$(dirname "$0")/.." && pwd)/docs/ROADMAP.md"
DRY_RUN="${DRY_RUN:-0}"

[ -f "$ROADMAP" ] || { echo "error: $ROADMAP not found" >&2; exit 1; }
command -v awk >/dev/null || { echo "error: awk required" >&2; exit 1; }
if [ "$DRY_RUN" != "1" ]; then
  command -v gh >/dev/null || { echo "error: gh required (or set DRY_RUN=1)" >&2; exit 1; }
  gh auth status >/dev/null 2>&1 || { echo "error: run 'gh auth login' first" >&2; exit 1; }
fi

run() { if [ "$DRY_RUN" = "1" ]; then echo "+ $*"; else "$@"; fi; }

echo ">> target repo: $REPO  (DRY_RUN=$DRY_RUN)"

# --- labels -----------------------------------------------------------------
ensure_label() { # name  color  description
  run gh label create "$1" --repo "$REPO" --color "$2" --description "$3" --force \
    >/dev/null 2>&1 || true
}
echo ">> ensuring labels"
ensure_label vanilla-parity  1d76db "Matches a feature in the official FFXI client"
ensure_label enhanced        5319e7 "Opt-in modernization with no retail analog"
ensure_label status:missing  b60205 "Not started"
ensure_label status:partial  fbca04 "Decoded or scaffolded; UI/dispatch incomplete"
ensure_label status:unknown  cccccc "Not yet investigated"
for a in world-rendering hud combat-action inventory-equipment party-social \
         world-interaction character-progression launcher-lobby enhanced-addon; do
  ensure_label "area:$a" 0e8a16 "Roadmap area: $a"
done

# --- parse roadmap into TSV: board \t area \t status \t title \t body --------
# Body newlines are encoded as the two-character sequence \n and decoded with
# printf '%b' below.
parse() {
  awk '
    function flush(   t) {
      if (initem && status != "x") {
        t = title
        gsub(/\*\*/, "", t); gsub(/`/, "", t)        # strip md emphasis
        sub(/[[:space:]]+$/, "", t)
        if (length(t) > 90) t = substr(t, 1, 88) "…"
        aa = area; if (aa == "") aa = "enhanced-addon"  # Enhanced has no ### subhead
        # \037 (unit separator) delimits fields so empty ones never collapse.
        printf "%s\037%s\037%s\037%s\037%s\n", board, aa, status, t, body
      }
      initem = 0; title = ""; body = ""; status = ""; titledone = 0
    }
    /^## / {
      flush()
      h = $0; sub(/^## /, "", h)
      if (h ~ /^Vanilla/) board = "vanilla-parity"
      else if (h ~ /^Enhanced/) board = "enhanced"
      else board = ""
      area = ""
      next
    }
    /^### / {
      flush()
      a = tolower($0); sub(/^### /, "", a)
      gsub(/&/, "", a); gsub(/[^a-z0-9]+/, "-", a)
      sub(/^-+/, "", a); sub(/-+$/, "", a)
      area = a
      next
    }
    /^---/ { flush(); next }
    /^[[:space:]]*$/ { flush(); next }
    /^- \[.\] / {                                    # top-level scoreboard item
      flush()
      if (board == "") next
      initem = 1
      status = substr($0, 4, 1)
      line = $0; sub(/^- \[.\] /, "", line)
      title = line
      body = "- [" status "] " line
      next
    }
    {
      if (initem) {                                  # continuation / sub-bullet
        ln = $0
        c = ln; sub(/^[[:space:]]+/, "", c)
        body = body "\\n" $0
        if (c ~ /^- \[.\]/ || c ~ /^- /) titledone = 1
        else if (!titledone) {                       # de-wrap prose into title
          sub(/^[[:space:]]+/, "", ln)
          title = title " " ln
        }
      }
    }
    END { flush() }
  ' "$ROADMAP"
}

# --- create issues ----------------------------------------------------------
created=0; skipped=0
while IFS=$'\037' read -r board area status title body; do
  [ -n "$title" ] || continue

  # idempotency: skip if a same-titled issue already exists (open or closed)
  if [ "$DRY_RUN" != "1" ]; then
    existing="$(gh issue list --repo "$REPO" --state all --search "in:title \"$title\"" \
                  --json title --jq "[.[] | select(.title == \"$title\")] | length" 2>/dev/null || echo 0)"
    if [ "${existing:-0}" != "0" ]; then
      echo "   skip (exists): $title"; skipped=$((skipped + 1)); continue
    fi
  fi

  printf -v decoded '%b' "$body"
  full="$decoded

---
Scoreboard: **$board** · Area: **$area** · Status: **$status**
Tracked in [docs/ROADMAP.md](https://github.com/$REPO/blob/main/docs/ROADMAP.md). When this lands, flip its glyph there in the same PR."

  case "$status" in
    "~") st=partial ;;
    "?") st=unknown ;;
    *)   st=missing ;;
  esac
  labels="$board,area:$area,status:$st"
  echo "   create: [$labels] $title"
  run gh issue create --repo "$REPO" --title "$title" --body "$full" --label "$labels" >/dev/null
  created=$((created + 1))
done < <(parse)

echo ">> done. created=$created skipped=$skipped"
[ "$DRY_RUN" = "1" ] && echo ">> (dry run — nothing was created)"
exit 0
