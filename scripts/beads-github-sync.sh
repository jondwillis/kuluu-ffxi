#!/usr/bin/env bash
# One-way GitHub Issues → beads sync.
#
# Pulls every issue from the `origin` GitHub repo and upserts it into beads,
# keyed on `external_ref: gh-<number>` so re-runs update in place instead of
# duplicating (bd import has upsert semantics). State maps OPEN→open,
# CLOSED→closed; GitHub labels carry over plus a `github` tag.
#
# This is GitHub → beads ONLY. It never writes to GitHub, so beads-origin
# issues (e.g. the `roadmap`-labelled seed) are not pushed up; publishing those
# for contributors would need a separate beads → GitHub direction.
#
#   scripts/beads-github-sync.sh            # sync now
#   scripts/beads-github-sync.sh --dry-run  # print the beads JSONL, import nothing
#
# Run it where the beads Dolt db lives (a working clone). See
# .github/workflows/beads-sync.yml for the opt-in CI variant.

set -euo pipefail

dry_run=0
[ "${1:-}" = "--dry-run" ] && dry_run=1

for tool in gh jq bd; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: '$tool' not found on PATH" >&2; exit 1; }
done

mapped=$(gh issue list --state all --limit 1000 \
  --json number,title,body,state,labels \
  | jq -c '.[] | {
      external_ref: ("gh-" + (.number | tostring)),
      title: .title,
      description: (.body // ""),
      status: (if .state == "CLOSED" then "closed" else "open" end),
      issue_type: "task",
      labels: ([.labels[].name] + ["github"])
    }')

count=$(printf '%s' "$mapped" | grep -c . || true)
if [ "$count" -eq 0 ]; then
  echo "No GitHub issues to sync."
  exit 0
fi

if [ "$dry_run" -eq 1 ]; then
  printf '%s\n' "$mapped"
  echo "(dry-run) would upsert $count issue(s) into beads" >&2
  exit 0
fi

printf '%s\n' "$mapped" | bd import -
echo "Synced $count GitHub issue(s) → beads (external_ref gh-<number>)."
