#!/usr/bin/env bash
# PostToolUse hook: after an Edit/Write to a workspace crate's source file, run
# that crate's lib tests so an assertion regression surfaces on the next turn
# instead of at the next push.
#
# Event, protocol, DAT, session and viewer edits run shared state contracts; other crates
# run their lib tests. Full builds stay outside this per-edit hook.
#
# WHY `-p <crate>` with no --features, even though checks.sh warns that a
# mismatched feature set forks the dependency graph: measured here, the fork is
# the cheap option. Timings for an edit to kuluu-render (bead kuluu-p5a5):
#     cargo test -p kuluu-render --lib .................    9s
#     cargo test --workspace --features native-window --lib .  254s
#     cargo check -p kuluu-render --lib ................   111s
# The forked graph costs disk (a second set of artifacts), not wall clock, once
# both sets exist. Matching the workspace unification would be ~28x slower per
# edit, so the narrow invocation stays.
#
# Runs through cargo-guard so a jobserver wedge can't turn one edit into a
# 20-minute stall — the original version of this hook had no timeout at all
# despite a comment claiming otherwise, which is exactly how a wedged tree
# became invisible.
#
# Exits 0 unconditionally: failures surface as stderr context for the agent,
# not as a blocked tool call. The point is fast feedback, not enforcement.

set -uo pipefail

# Measured at 9s for the largest crate; this leaves ample headroom while still
# bounding a pathological run far below a conversation turn.
readonly HOOK_TIMEOUT_SECS=180
readonly HOOK_STALL_SECS=60
# Long enough to collapse a burst of edits to one crate, short enough that a
# later edit in the same turn still gets a fresh answer.
readonly DEBOUNCE_SECS=90

payload=$(cat)
file=$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(d.get("tool_input",{}).get("file_path",""), end="")' \
  2>/dev/null || true)

[ -n "$file" ] || exit 0

# The `|` delimiter must not be `|`: an alternation inside the pattern then
# terminates the s/// early and BSD sed aborts with "parentheses not balanced",
# leaving $crate empty so the hook silently no-ops. It shipped that way and
# never once ran on macOS.
crate=$(printf '%s' "$file" | sed -nE 's#.*/((ffxi|kuluu)-[^/]+|kuluu)/(src|tests)/.*#\1#p')
[ -n "$crate" ] || exit 0

repo=$(git -C "$(dirname "$file")" rev-parse --show-toplevel 2>/dev/null) || exit 0
guard="$repo/scripts/cargo-guard.sh"
[ -x "$guard" ] || exit 0

# Agents edit the same crate several times in a row; re-running its suite after
# each one costs the same 9-18s to re-learn the same answer. One run per crate
# per window is the same signal at a fraction of the tax.
stamp="${TMPDIR:-/tmp}/kuluu-affected-tests-$crate.stamp"
now_secs=$(date +%s)
if [ -f "$stamp" ]; then
  last=$(cat "$stamp" 2>/dev/null || echo 0)
  [ $((now_secs - ${last:-0})) -lt "$DEBOUNCE_SECS" ] && exit 0
fi
printf '%s' "$now_secs" > "$stamp"

case "$crate" in
  ffxi-event|ffxi-proto|ffxi-dat|ffxi-vocab|kuluu-session|kuluu-render|kuluu-snapshot|kuluu)
    output=$(cd "$repo" && CARGO_GUARD_TIMEOUT=$HOOK_TIMEOUT_SECS \
      CARGO_GUARD_STALL=$HOOK_STALL_SECS scripts/checks.sh contracts 2>&1)
    status=$?
    ;;
  *)
    output=$(CARGO_GUARD_TIMEOUT=$HOOK_TIMEOUT_SECS \
      CARGO_GUARD_STALL=$HOOK_STALL_SECS \
      "$guard" test -p "$crate" --lib --quiet 2>&1)
    status=$?
    ;;
esac

case "$status" in
  124)
    cat >&2 <<MSG
[affected-crate-tests] timed out after ${HOOK_TIMEOUT_SECS}s on $crate — skipped.
Another cargo invocation is probably holding the build lock.
MSG
    ;;
  125)
    cat >&2 <<MSG
[affected-crate-tests] cargo wedged (no CPU progress) while testing $crate.
$(printf '%s' "$output" | grep 'cargo-guard:' | head -8)
MSG
    ;;
  0) ;;
  *)
    cat >&2 <<MSG
[affected-crate-tests] $crate checks failed after edit to $file:
$(printf '%s' "$output" | tail -10)
MSG
    ;;
esac

exit 0
