#!/usr/bin/env bash
# Write the verification evidence marker (.verify/latest.json) that the
# stop-hook verify gate (stop.d/25-verify.sh) checks. Run this as the LAST
# step of a /verify session, after the evidence files exist.
#
#   record-evidence.sh --verdict pass|fail|waived --summary "<observed>" \
#       [--artifact path]...
#
# Contract enforced here (so the gate's spot check rarely has to fire):
#   - verdict pass REQUIRES at least one artifact
#   - every artifact must exist and be non-empty at record time
#   - waived REQUIRES a summary explaining why runtime verification
#     does not apply (the gate shows it to the user)
# The marker is timestamped at write; the gate treats it as stale the
# moment any gated source file is edited after it.

set -euo pipefail

usage() { sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }

verdict="" summary=""
artifacts=()
while [ $# -gt 0 ]; do
  case "$1" in
    --verdict)  verdict=${2:?}; shift 2 ;;
    --summary)  summary=${2:?}; shift 2 ;;
    --artifact) artifacts+=("${2:?}"); shift 2 ;;
    *) printf 'unknown arg: %s\n' "$1" >&2; usage ;;
  esac
done

case "$verdict" in pass|fail|waived) ;; *) printf 'verdict must be pass|fail|waived\n' >&2; usage ;; esac
[ -z "$summary" ] && { printf -- '--summary is required: record what was OBSERVED, not what was done\n' >&2; exit 1; }
if [ "$verdict" = pass ] && [ ${#artifacts[@]} -eq 0 ]; then
  printf 'verdict pass requires at least one --artifact (events.jsonl, screenshot, log)\n' >&2; exit 1
fi

repo=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
for a in ${artifacts[@]+"${artifacts[@]}"}; do
  [ -s "$a" ] || [ -s "$repo/$a" ] || { printf 'artifact missing or empty: %s\n' "$a" >&2; exit 1; }
done

mkdir -p "$repo/.verify"
now_epoch=$(date +%s)
jq -n \
  --arg verdict "$verdict" \
  --arg summary "$summary" \
  --arg iso "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson epoch "$now_epoch" \
  --arg head "$(git -C "$repo" rev-parse HEAD 2>/dev/null || echo unknown)" \
  --args '{
    verdict: $verdict,
    summary: $summary,
    verified_at: $iso,
    verified_at_epoch: $epoch,
    head: $head,
    artifacts: $ARGS.positional
  }' -- ${artifacts[@]+"${artifacts[@]}"} > "$repo/.verify/latest.json"

printf 'recorded %s → %s\n' "$verdict" "$repo/.verify/latest.json"
