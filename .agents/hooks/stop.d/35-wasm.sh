#!/usr/bin/env bash
# Stop sub-check (priority 35): the browser viewer is the one CI target no
# local workflow builds, so an item gated out of wasm32 but referenced from
# ungated code compiles natively all session and only fails at push (or in
# CI). The pre-push gate catches it, but by then the break is already
# committed and the fix is a follow-up commit on a red branch.
#
# Only kuluu-render carries those cfg gates; kuluu-snapshot and
# kuluu-viewer-wasm are the wire type it renders from and the crate under
# check, matching the trees .githooks/pre-push diffs.
#
# Escape hatch: WASM_STOP_CHECK=off.
#
# Contract: see stop-lib.sh. Exit 0 = pass; fire = block with the reason.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=../stop-lib.sh
. "$here/../stop-lib.sh"
load_payload

[ "${WASM_STOP_CHECK:-on}" = "off" ] && exit 0

TREES=(kuluu-render kuluu-snapshot kuluu-viewer-wasm)

cd "$CWD" 2>/dev/null || exit 0
[ -x "scripts/checks.sh" ] || exit 0

# Both halves of "not yet on the remote": edits still in the working tree
# and commits already made but unpushed. Either can carry the break.
changes=$(
  git status --porcelain -- "${TREES[@]}" 2>/dev/null
  git diff --name-only '@{push}..' -- "${TREES[@]}" 2>/dev/null
)
[ -n "$changes" ] || exit 0

# Re-running cargo for a tree that has not moved since the last check buys
# nothing -- and this fires on every Stop, so the wasted minutes compound.
state=$(printf '%s' "$changes"; git diff -- "${TREES[@]}" 2>/dev/null)
sig=$(printf '%s' "$state" | shasum | cut -d' ' -f1)
snap_dir="${TMPDIR:-/tmp}/claude-stop-wasm"
mkdir -p "$snap_dir"
snap="$snap_dir/${SESSION_ID:-unknown}.sig"
[ -f "$snap" ] && [ "$(cat "$snap")" = "$sig" ] && exit 0

log=$(mktemp)
if scripts/checks.sh wasm >"$log" 2>&1; then
  printf '%s' "$sig" > "$snap"
  rm -f "$log"
  exit 0
fi

# A nonzero exit carrying no rustc diagnostic is the environment, not the
# diff: EAGAIN, or another cargo already holding the build lock (this repo
# runs concurrent agent sessions and a pre-push gate against one target/).
# Blaming the code for those is the misreport this check exists to avoid,
# and recording the signature would suppress the real check next Stop.
# A jobserver stall kills rustc mid-crate, and cargo then reports the corpse as
# "error: could not compile" -- a diagnostic shape indistinguishable from a real
# break, so the wedge has to be recognised before the diagnostics are read.
if grep -q 'WEDGE DETECTED' "$log"; then
  rm -f "$log"
  exit 0
fi

# "os error 35" is EAGAIN, which cargo also prints on an `error:` line, so
# the environment lines have to be dropped before the diagnostic test --
# otherwise the very failure this guard exists for reads as a compile error.
errors=$(grep -E '^error' "$log" \
  | grep -vE 'os error 35|Resource temporarily unavailable|file lock' \
  | head -20)
rm -f "$log"
[ -n "$errors" ] || exit 0

printf '%s' "$sig" > "$snap"
fire "The wasm32 build of the browser viewer is broken by the changes in ${TREES[*]}:

$errors

An item is most likely gated out of wasm32 (#[cfg(not(target_arch = \"wasm32\"))]) but still referenced from ungated code. Fix it before this reaches the pre-push gate, then re-run: scripts/checks.sh wasm"
