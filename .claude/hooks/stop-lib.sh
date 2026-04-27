#!/usr/bin/env bash
# Shared helpers for the Stop sub-checks under stop.d/. Source this; do
# not execute it.
#
# Contract for a stop.d/NN-*.sh check (run by stop-dispatcher.sh):
#   - reads the raw Stop payload JSON on stdin (call load_payload)
#   - exits 0 to PASS (let the next check run)
#   - calls fire "<reason>" to FIRE: prints the reason and exits 10, at
#     which point the dispatcher wraps it into a `decision: block` and
#     stops iterating. First firing check wins; lower-priority checks
#     never run, so only the winner ever records its signature.
# The dispatcher owns the stop_hook_active loop guard, so checks don't
# repeat it.

FIRE=10

# load_payload: slurp stdin JSON once, expose the common fields.
load_payload() {
  PAYLOAD=$(cat)
  SESSION_ID=$(printf '%s' "$PAYLOAD" | jq -r '.session_id // empty')
  CWD=$(printf '%s' "$PAYLOAD" | jq -r '.cwd // empty')
  TRANSCRIPT=$(printf '%s' "$PAYLOAD" | jq -r '.transcript_path // empty')
  [ -z "$CWD" ] && CWD="$PWD"
}

# fire <reason>: emit the reason and exit with the FIRE code.
fire() { printf '%s' "$1"; exit "$FIRE"; }

# sig_changed <snap-subdir> <signature>: return 0 (and record the new
# signature) when it differs from the stored one for this session; 1 when
# unchanged. Side-effecting — call it only at the point of firing, so a
# check that passes never bumps its signature.
sig_changed() {
  local snap_dir="${TMPDIR:-/tmp}/$1" sig_file
  [ -n "$SESSION_ID" ] || return 1
  mkdir -p "$snap_dir"
  sig_file="$snap_dir/${SESSION_ID}.sig"
  [ -f "$sig_file" ] && [ "$(cat "$sig_file")" = "$2" ] && return 1
  printf '%s' "$2" > "$sig_file"
  return 0
}
