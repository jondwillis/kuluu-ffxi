#!/usr/bin/env bash
# Stop sub-check (priority 25, after commit-nudge): runtime-observable
# source changed this session but no verification evidence backs it —
# insist on /verify before the session settles.
#
# Layered, cheapest-first:
#   1. Marker + spot check (deterministic): .verify/latest.json written by
#      record-evidence.sh must postdate the last source change AND its
#      artifact paths must exist non-empty. Catches "never verified" and
#      "verified, then kept editing".
#   2. Transcript inspection (deterministic): no fresh marker, but the
#      transcript shows real verification activity after the last source
#      change (MCP drive, event captures, live tests) — covers marker
#      omission without false-blocking.
#   3. Agent judge (model call, rare): only in the thin-evidence band —
#      fresh marker with bogus artifacts, or transcript signals that need
#      reading, not grepping. `claude -p` haiku, PASS/FAIL contract.
#
# Escape hatches: VERIFY_GATE=off env; verdict "waived" in the marker
# (still must be fresh — a stale waiver doesn't cover new edits).
# Doc/hook/test-only changes never trip the gate (source filter below).
#
# Contract: see stop-lib.sh. Exit 0 = pass; fire = exit 10 + reason.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=../stop-lib.sh
. "$here/../stop-lib.sh"
load_payload

[ "${VERIFY_GATE:-on}" = "off" ] && exit 0
git -C "$CWD" rev-parse --git-dir >/dev/null 2>&1 || exit 0

snap="${TMPDIR:-/tmp}/claude-verify-gate/${SESSION_ID}"
[ -f "$snap.head" ] || exit 0  # no baseline → can't attribute changes to this session
base_head=$(cat "$snap.head")

# --- What runtime-observable source did THIS session change? -------------
# Committed during session (baseline HEAD..HEAD) + dirtied beyond the
# SessionStart porcelain snapshot. Filter to surfaces /verify can observe:
# Rust + shaders, excluding tests (covered by cargo, not runtime drive)
# and vendor/ (LSB upstream, verified by its own suite).
committed=$(git -C "$CWD" diff --name-only "$base_head"..HEAD 2>/dev/null || true)

current=$(git -C "$CWD" status --porcelain 2>/dev/null || true)
session_dirty=$(comm -23 \
  <(printf '%s\n' "$current" | sort -u) \
  <(sort -u "$snap.porcelain" 2>/dev/null) \
  | sed -E 's/^.{3}//; s/^"(.*)"$/\1/; s/.* -> //' || true)  # strip status cols, quotes, rename arrows

changed=$(printf '%s\n%s\n' "$committed" "$session_dirty" \
  | grep -E '\.(rs|wgsl)$' \
  | grep -Ev '(^|/)tests?/|^vendor/' \
  | sort -u | grep -v '^$' || true)
[ -z "$changed" ] && exit 0

# Last-change epoch = max mtime of the changed files still on disk.
last_change=0
while IFS= read -r f; do
  [ -f "$CWD/$f" ] || continue
  m=$(stat -f %m "$CWD/$f" 2>/dev/null || echo 0)
  [ "$m" -gt "$last_change" ] && last_change=$m
done <<< "$changed"
[ "$last_change" -eq 0 ] && exit 0  # everything deleted → nothing observable

# --- Layer 1: evidence marker + artifact spot check ----------------------
marker="$CWD/.verify/latest.json"
marker_fresh=false thin_reason=""
if [ -f "$marker" ]; then
  verified_epoch=$(jq -r '.verified_at_epoch // 0' "$marker" 2>/dev/null || echo 0)
  verdict=$(jq -r '.verdict // empty' "$marker" 2>/dev/null || true)
  if [ "${verified_epoch:-0}" -ge "$last_change" ]; then
    marker_fresh=true
    [ "$verdict" = "waived" ] && exit 0  # fresh waiver: user opted out for this change
    bad=""
    while IFS= read -r a; do
      [ -z "$a" ] && continue
      [ -s "$a" ] || [ -s "$CWD/$a" ] || bad="$bad $a"
    done < <(jq -r '.artifacts[]? // empty' "$marker" 2>/dev/null)
    artifact_count=$(jq -r '.artifacts | length' "$marker" 2>/dev/null || echo 0)
    if [ -z "$bad" ] && [ "${artifact_count:-0}" -gt 0 ]; then
      exit 0  # fresh marker, real artifacts — verified
    fi
    thin_reason="marker is fresh but its evidence is thin (artifacts:${artifact_count:-0}, missing/empty:${bad:-none})"
  fi
fi

# --- Layer 2: transcript inspection --------------------------------------
# Verification leaves tool-call fingerprints: MCP drive, event/log capture,
# live integration tests, GUI screenshots, the evidence recorder itself.
# Count only activity AFTER the last source change — earlier runs verified
# an earlier tree.
SIG_RE='ffxi-mcp|events\.jsonl|play --headless|play_lifecycle|zone_change|agent_session|screencapture|record-evidence\.sh|hxi\.sh (capture|key|click|type)'
last_sig=0
if [ -n "$TRANSCRIPT" ] && [ -f "$TRANSCRIPT" ]; then
  last_sig=$(jq -rs --arg re "$SIG_RE" '
    [ .[]
      | select(.timestamp? and ((.message.content? // "") | tostring | test($re)))
      | (.timestamp | sub("\\.[0-9]+Z$"; "Z") | fromdate? // 0)
    ] | max // 0' "$TRANSCRIPT" 2>/dev/null || echo 0)
  last_sig=${last_sig%%.*}  # jq may emit a float
fi

if [ "$marker_fresh" = false ] && [ "${last_sig:-0}" -ge "$last_change" ]; then
  thin_reason="no evidence marker, but the transcript shows verification-shaped activity after the last edit"
fi

# --- Fire signature (same-content loop guard) -----------------------------
sig=$( { printf '%s\n' "$changed"; printf '%s %s\n' "$last_change" "$(stat -f %m "$marker" 2>/dev/null || echo 0)"; } \
  | shasum -a 256 | cut -d' ' -f1)

# --- Layer 3: agent judge, only in the thin band --------------------------
if [ -n "$thin_reason" ]; then
  judge_bin="${VERIFY_JUDGE_BIN:-claude}"
  if command -v "$judge_bin" >/dev/null 2>&1 && [ -n "$TRANSCRIPT" ] && [ -f "$TRANSCRIPT" ]; then
    verdict_out=$(tail -c 200000 "$TRANSCRIPT" | perl -e 'alarm 90; exec @ARGV' -- \
      "$judge_bin" -p --model haiku --max-turns 1 \
      "You are a verification auditor for a game-client repo. Input: the tail of a session transcript (JSONL). The session changed these source files: $(printf '%s ' $changed). Judge STRICTLY whether the assistant actually runtime-verified those changes: drove the client or server live, and OBSERVED concrete evidence (event-stream lines, tracing/log lines, screenshots, live-test output) that speaks to the changed behavior. Claims without captured evidence = FAIL. Tests merely compiling = FAIL. Reply with exactly one line: PASS or FAIL: <short reason>." \
      2>/dev/null || true)
    case "$verdict_out" in
      *PASS*) exit 0 ;;
      *FAIL*) jr=${verdict_out#*FAIL}; thin_reason="$thin_reason; judge: ${jr#: }" ;;
      *)      thin_reason="$thin_reason; judge unavailable — treating thin evidence as unverified" ;;
    esac
  fi
  sig_changed claude-verify-gate "$sig" || exit 0
  fire "$(printf 'Verification gate: %s.\n\nChanged this session:\n%s\n\nRun the /verify skill against the live stack for these changes, then record the evidence:\n  .claude/skills/verify/scripts/record-evidence.sh --verdict pass --summary "<what was observed>" --artifact <events.jsonl|screenshot|log> [...]\nIf runtime verification genuinely does not apply, record --verdict waived --summary "<why>". Do not claim verification without artifacts.' \
    "$thin_reason" "$changed")"
fi

sig_changed claude-verify-gate "$sig" || exit 0
fire "$(printf 'Verification gate: runtime-observable source changed this session with NO verification evidence.\n\nChanged:\n%s\n\nRun the /verify skill (headless MCP drive or GUI attach — see the surface table) and capture evidence, then record it:\n  .claude/skills/verify/scripts/record-evidence.sh --verdict pass --summary "<what was observed>" --artifact <events.jsonl|screenshot|log> [...]\nIf runtime verification genuinely does not apply to these files, record --verdict waived --summary "<why>". A waiver must postdate the last edit.' \
  "$changed")"
