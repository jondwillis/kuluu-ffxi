#!/usr/bin/env bash
# Stop sub-check (priority 40, bookkeeping): catch ROADMAP <-> beads drift
# in-session, before CI does. roadmap_sync.py --check exits 1 only on HARD
# drift (a [x] line whose roadmap-labelled bead isn't closed, or a closed bead
# whose line isn't [x]) — the same class ci enforces. Soft drift ([ ] vs [~])
# and unmatched beads are warnings the script never blocks on, so neither do we.
#
# We nudge, we don't auto-fix: the [x]->[~] downgrade is a human call (verify
# the bead's grounding), and the safe closed->[x] direction is `--write`. Lowest
# priority so real concerns (questions, commits, comment-rot) surface first;
# sig-guarded so it fires once per distinct drift set, not every Stop.
#
# Contract: see stop-lib.sh. Exit 0 = pass; fire = exit 10 + reason.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=../stop-lib.sh
. "$here/../stop-lib.sh"
load_payload

git -C "$CWD" rev-parse --git-dir >/dev/null 2>&1 || exit 0
command -v python3 >/dev/null 2>&1 || exit 0
script="$CWD/scripts/roadmap_sync.py"
[ -f "$script" ] || exit 0
[ -f "$CWD/docs/ROADMAP.md" ] || exit 0
[ -f "$CWD/.beads/issues.jsonl" ] || exit 0

# The script resolves its own repo ROOT from __file__, so CWD is irrelevant.
out=$(python3 "$script" --check 2>/dev/null)
rc=$?
[ "$rc" -eq 1 ] || exit 0  # 0 = no hard drift; 2 = usage (shouldn't happen)

drift=$(printf '%s\n' "$out" | grep -E '\]->\[' || true)
[ -z "$drift" ] && exit 0
count=$(printf '%s\n' "$drift" | grep -c .)

# Fire once per distinct drift set; a changed/resolved set re-evaluates.
sig=$(printf '%s' "$drift" | shasum -a 256 | cut -d' ' -f1)
sig_changed claude-roadmap-sync "$sig" || exit 0

fire "$(printf 'Stop-hook checkpoint: ROADMAP <-> beads drift — %s hard-drift line(s) where the glyph disagrees with the roadmap-labelled bead status:\n%s\n\nReconcile before finishing: for each, either close the bead (if the feature is genuinely done) or downgrade the ROADMAP line to match the bead ([x]->[~] or [x]->[ ]). The safe closed->[x] direction is `python3 scripts/roadmap_sync.py --write`; the downgrade is a human call — verify the bead grounding (bd show) first. Then re-run `python3 scripts/roadmap_sync.py --check` (must exit 0). Quiet until the drift set changes.' \
  "$count" "$drift")"
