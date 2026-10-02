#!/usr/bin/env bash
# Attribution tests for the PreToolUse/PostToolUse(Bash) edit-ledger hooks.
#
#   bash .agents/hooks/tests/session-edits-bash-attribution.test.sh
#
# Every case gets a throwaway git repo and points TMPDIR at it before any
# hook runs, so the operator's live session ledgers are never read or written.

set -uo pipefail

HOOKS=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../session-edits.lib.sh
. "$HOOKS/session-edits.lib.sh"
# For FIRE, the stop-check exit code: re-typing 10 here would be a second
# source for a contract the dispatcher and every stop.d check share.
# shellcheck source=../stop-lib.sh
. "$HOOKS/stop-lib.sh"

FAILURES=0
CASE=""

fail() { printf 'FAIL - %s: %s\n' "$CASE" "$1"; FAILURES=$((FAILURES + 1)); }

# Git hooks export repository selectors that would redirect fixture commands into the caller.
while IFS= read -r git_local_var; do
  unset "$git_local_var"
done < <(git rev-parse --local-env-vars)

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT
export HOME="$ROOT/home"
mkdir -p "$HOME"

# The fixture repo is built once and copied per case: git init plus a commit
# dominates this suite's runtime, and checks.sh harness runs it on every push.
TEMPLATE="$ROOT/template"
mkdir -p "$TEMPLATE/src" "$TEMPLATE/hud"
git -C "$TEMPLATE" init -q
git -C "$TEMPLATE" config user.email tester@example.invalid
git -C "$TEMPLATE" config user.name tester
printf 'alpha\n' > "$TEMPLATE/src/a.txt"
printf 'aabb\n' > "$TEMPLATE/src/ab.txt"
printf 'one\n' > "$TEMPLATE/src/b.txt"
printf 'mod\n' > "$TEMPLATE/hud/mod.rs"
printf 'root mod\n' > "$TEMPLATE/mod.rs"
printf 'readme\n' > "$TEMPLATE/README.md"
mkdir -p "$TEMPLATE/.beads"
printf '{"id":"seed"}\n' > "$TEMPLATE/.beads/issues.jsonl"
git -C "$TEMPLATE" add src README.md hud mod.rs .beads
git -C "$TEMPLATE" commit -qm init

# Named for the case, not a counter: cases run in concurrent subshells, so a
# shared counter would hand every one of them the same sandbox.
new_repo() {
  SANDBOX="$ROOT/sandbox-$CASE"
  rm -rf "$SANDBOX"
  export TMPDIR="$SANDBOX/tmp"
  mkdir -p "$TMPDIR"
  REPO="$SANDBOX/repo"
  cp -Rp "$TEMPLATE" "$REPO"
  SID="sess-a"
}

payload() {
  jq -nc --arg sid "$1" --arg cwd "$REPO" --arg cmd "$2" \
    '{session_id: $sid, cwd: $cwd, tool_input: {command: $cmd}}'
}

# run_hook <script> <payload>: assert the never-blocks/never-speaks contract
# on every invocation, since nothing else pins it.
run_hook() {
  local out err rc
  err="$SANDBOX/stderr"
  out=$(printf '%s' "$2" | "$HOOKS/$1" 2>"$err")
  rc=$?
  [ "$rc" = 0 ] || fail "$1 exited $rc"
  [ -z "$out" ] || fail "$1 wrote to stdout: $out"
  [ -s "$err" ] && fail "$1 wrote to stderr: $(cat "$err")"
  return 0
}

payload_at() {
  jq -nc --arg sid "$1" --arg cwd "$2" --arg cmd "$3" \
    '{session_id: $sid, cwd: $cwd, tool_input: {command: $cmd}}'
}

ledger_has() { ledger_read "$1" | grep -qxF "$2"; }
suspect_has() { [ -f "$(suspect_path "$1")" ] && cut -f1 "$(suspect_path "$1")" | grep -qxF "$2"; }

assert_in_ledger() { ledger_has "$1" "$2" || fail "$2 missing from $1 ledger"; }
assert_not_in_ledger() { ledger_has "$1" "$2" && fail "$2 wrongly credited to $1 ledger"; return 0; }
assert_in_suspect() { suspect_has "$1" "$2" || fail "$2 missing from $1 suspect log"; }

snap_files() { find "$(snap_dir)" -type f 2>/dev/null | wc -l | tr -d ' '; }

# The race this suite exists for: a peer session writes while a command that
# neither names the path nor has a writer form is in flight.
test_peer_write_not_attributed() {
  new_repo
  local p; p=$(payload "$SID" "wc -l README.md")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/a.txt
}

test_peer_write_recorded_as_suspect() {
  new_repo
  local p; p=$(payload "$SID" "wc -l README.md")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_suspect "$SID" src/a.txt
}

# The write a session really did make. The on-disk mutation uses printf so the
# case behaves the same on BSD and GNU, while the command TEXT still exercises
# both the naming arm and the sed -i writer arm.
test_named_sed_write_attributed() {
  new_repo
  local p; p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

# Already dirty before the command, so its porcelain line never changes and
# the delta cannot see the second edit. Only the signature arm credits it.
test_named_write_to_already_dirty_path_attributed() {
  new_repo
  printf 'two\n' > "$REPO/src/b.txt"
  local p; p=$(payload "$SID" "sed -i '' 's/two/three/' src/b.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'three\n' > "$REPO/src/b.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/b.txt
}

# A peer's `git add` flips the porcelain status columns, manufacturing a delta
# line for a file nobody's command wrote; the content check kills it.
test_named_but_unchanged_path_not_attributed() {
  new_repo
  printf 'two\n' > "$REPO/src/b.txt"
  local p; p=$(payload "$SID" "wc -l src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  git -C "$REPO" add src/b.txt
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/a.txt
  assert_not_in_ledger "$SID" src/b.txt
}

# A bare writer form with no file operand still credits a real reformat: the
# plausible set for cargo fmt/fix is the workspace's tracked *.rs, and
# dropping this arm would silence the commit nudge on work that is ours.
test_writer_form_without_named_path_attributed() {
  new_repo
  local p; p=$(payload "$SID" "cargo fmt --all")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'formatted\n' > "$REPO/mod.rs"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" mod.rs
}

# The race closed for non-writer commands: a peer's write to a path cargo
# fmt cannot touch, concurrent with a bare cargo fmt --all, must land in the
# suspect log, not the ledger.
test_peer_write_outside_fmt_plausible_set_is_suspect() {
  new_repo
  local p; p=$(payload "$SID" "cargo fmt --all")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_suspect "$SID" src/a.txt
}

# cargo fmt formats the workspace's tracked sources; an untracked .rs file a
# peer drops into the tree during the window is not in the plausible set.
test_untracked_rs_outside_fmt_plausible_set_is_suspect() {
  new_repo
  local p; p=$(payload "$SID" "cargo fmt --all")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' > "$REPO/src/peer.rs"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/peer.rs
  assert_in_suspect "$SID" src/peer.rs
}

# rmcm names its operands, so only its operands are plausible (a directory
# operand reaches into its tree); a peer write elsewhere in the window is a
# suspect.
test_rmcm_plausible_set_is_its_operands() {
  new_repo
  local p; p=$(payload "$SID" "rmcm hud")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'formatted\n' > "$REPO/hud/mod.rs"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" hud/mod.rs
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_suspect "$SID" src/a.txt
}

# Without this carve-out almost every command is a writer form and the whole
# gate is cosmetic.
test_redirect_to_dev_null_is_not_a_writer_form() {
  new_repo
  local p; p=$(payload "$SID" "ls -la > /dev/null")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/a.txt
}

test_redirect_into_tree_is_a_writer_form() {
  new_repo
  local p; p=$(payload "$SID" "printf 'x' >> out.log")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'x\n' > "$REPO/out.log"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" out.log
}

# Pins the snapshot key agreement between pre and post: one byte of drift and
# post silently misses its own snapshot, making every other case pass for the
# wrong reason. Also pins that the .sig sibling is cleaned up, not leaked.
test_snapshot_key_matches_between_pre_and_post() {
  new_repo
  local p; p=$(payload "$SID" "wc -l README.md")
  [ "$(snap_files)" = 0 ] || fail "snapshot dir not empty before pre"
  run_hook session-edits-bash-pre.sh "$p"
  [ "$(snap_files)" = 2 ] || fail "pre wrote $(snap_files) snapshot files, want 2"
  run_hook session-edits-bash-post.sh "$p"
  [ "$(snap_files)" = 0 ] || fail "post leaked $(snap_files) snapshot files"
}

# A denied or interrupted Bash call leaves pre's pair with no post to consume
# it; without a sweep they accumulate for the life of the temp dir.
test_stale_snapshot_pair_is_swept() {
  new_repo
  local stale fresh
  run_hook session-edits-bash-pre.sh "$(payload "$SID" "wc -l README.md")"
  stale=$(snap_path "$SID" "wc -l README.md")
  touch -t 202001010000 "$stale" "$stale.sig"
  run_hook session-edits-bash-pre.sh "$(payload "$SID" "wc -l src/a.txt")"
  [ -f "$stale" ] && fail "stale snapshot survived the sweep"
  [ -f "$stale.sig" ] && fail "stale snapshot signature survived the sweep"
  fresh=$(snap_path "$SID" "wc -l src/a.txt")
  [ -f "$fresh" ] || fail "sweep took the live snapshot"
  return 0
}

# A dead session's ledger and suspect log would accumulate for the life of the
# temp dir, and a stale <sid>.suspect keeps counting into the commit nudge if
# the session id is reused. The sweep's policy is mtime-based: a file is reaped
# once its owner's last hook touch is older than the ledger TTL, and every hook
# touch refreshes that mtime, so the backdated touch stands in for a dead
# session.
test_stale_ledger_is_swept() {
  new_repo
  local p lp sp
  p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  p=$(payload "$SID" "wc -l README.md")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/b.txt"
  run_hook session-edits-bash-post.sh "$p"
  lp=$(ledger_path "$SID")
  sp=$(suspect_path "$SID")
  [ -f "$lp" ] || fail "ledger missing before the sweep"
  [ -f "$sp" ] || fail "suspect log missing before the sweep"
  touch -t 202001010000 "$lp" "$sp"
  run_hook session-edits-bash-pre.sh "$(payload "sess-b" "wc -l README.md")"
  [ -f "$lp" ] && fail "stale ledger survived the sweep"
  [ -f "$sp" ] && fail "stale suspect log survived the sweep"
  return 0
}

# The mtime refreshes on every tool call, not only on writes: a live session
# mid-way through a long read-only phase must not lose its ledger to the
# sweep.
test_live_ledger_survives_the_sweep() {
  new_repo
  local p lp
  p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  lp=$(ledger_path "$SID")
  touch -t 202001010000 "$lp"
  run_hook session-edits-bash-pre.sh "$(payload "$SID" "wc -l README.md")"
  [ -f "$lp" ] || fail "sweep took the live session's ledger"
  return 0
}

# Same guard as the snapshot TTL: an operator's junk ledger-TTL override must
# not turn a hook into a talker.
test_malformed_ledger_ttl_override_stays_silent() {
  new_repo
  export SESSION_EDITS_LEDGER_TTL=not-a-number
  local p; p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

# stat_shim <flavour>: put a stand-in `stat` first on PATH. "gnu" rejects the
# BSD -f format the way GNU stat does, so a Darwin runner can still exercise
# the Linux path; "none" fails outright.
stat_shim() {
  mkdir -p "$SANDBOX/bin"
  if [ "$1" = gnu ]; then
    cat > "$SANDBOX/bin/stat" <<'SH'
#!/bin/sh
[ "$1" = "-c" ] && [ "$2" = "%Y" ] || exit 1
exec date +%s
SH
  else
    cat > "$SANDBOX/bin/stat" <<'SH'
#!/bin/sh
exit 1
SH
  fi
  chmod +x "$SANDBOX/bin/stat"
  PATH="$SANDBOX/bin:$PATH"
}

# An unreadable mtime reads as infinitely stale, so a single stat spelling
# does not merely lose precision on the other platform - it discards every
# snapshot and disables Bash attribution outright.
test_portable_snapshot_mtime() {
  new_repo
  local m
  m=$(file_mtime "$REPO/src/a.txt")
  case "$m" in ''|*[!0-9]*) fail "file_mtime returned non-numeric '$m'"; return 0 ;; esac
  [ "$m" -gt 0 ] || fail "file_mtime returned $m for an existing file"
  stat_shim gnu
  m=$(file_mtime "$REPO/src/a.txt")
  case "$m" in ''|*[!0-9]*) m=0 ;; esac
  [ "$m" -gt 0 ] || fail "file_mtime ignored the GNU stat fallback, returned '$m'"
  stat_shim none
  m=$(file_mtime "$REPO/src/a.txt")
  [ "$m" = 0 ] || fail "file_mtime with no usable stat returned '$m', want 0"
}

# The whole feature, not just the helper: on a runner whose stat has no -f
# format the post-hook must still credit a named write rather than treat its
# own fresh snapshot as expired.
test_attribution_survives_gnu_only_stat() {
  new_repo
  stat_shim gnu
  local p; p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

# run_hook pins silence on every call, so this case is really about the sweep's
# arithmetic: an operator's junk TTL must not turn a hook into a talker.
test_malformed_ttl_override_stays_silent() {
  new_repo
  export SESSION_EDITS_SNAP_TTL=not-a-number
  local p; p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

# Pins that pre derives its filename from the library helper rather than a
# re-typed copy of the hash recipe.
test_snapshot_key_is_single_sourced() {
  new_repo
  local cmd="wc -l README.md" want
  run_hook session-edits-bash-pre.sh "$(payload "$SID" "$cmd")"
  want=$(snap_path "$SID" "$cmd")
  [ -f "$want" ] || fail "pre did not write the path snap_path names"
  [ -f "$want.sig" ] || fail "pre did not write the signature sibling"
}

# git hash-object has nothing to hash for a path the command removed, so only
# the absent sentinel separates a delete from "unchanged".
test_deleted_named_path_is_attributed() {
  new_repo
  local p; p=$(payload "$SID" "python3 -c \"import os; os.remove('src/a.txt')\"")
  run_hook session-edits-bash-pre.sh "$p"
  rm -f "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

test_ledger_forget_removes_only_named_path() {
  new_repo
  ledger_add "$SID" "$REPO" src/a.txt src/ab.txt hud/mod.rs
  "$HOOKS/session-edits-forget.sh" --session "$SID" --cwd "$REPO" src/a.txt
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_ledger "$SID" src/ab.txt
  assert_in_ledger "$SID" hud/mod.rs
}

test_ledger_forget_scoped_to_one_session() {
  new_repo
  ledger_add "$SID" "$REPO" src/a.txt
  ledger_add sess-b "$REPO" src/a.txt
  "$HOOKS/session-edits-forget.sh" --session "$SID" --cwd "$REPO" src/a.txt
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_ledger sess-b src/a.txt
}

test_commit_nudge_mentions_forget_helper() {
  new_repo
  printf 'edited\n' > "$REPO/src/a.txt"
  ledger_add "$SID" "$REPO" src/a.txt
  mkdir -p "$TMPDIR/claude-commit-nudge"
  : > "$TMPDIR/claude-commit-nudge/$SID.porcelain"
  local out rc
  out=$(jq -nc --arg sid "$SID" --arg cwd "$REPO" '{session_id: $sid, cwd: $cwd}' \
    | "$HOOKS/stop.d/20-commit.sh" 2>/dev/null)
  rc=$?
  [ "$rc" = "$FIRE" ] || fail "commit nudge exited $rc, want $FIRE (fire)"
  printf '%s' "$out" | grep -q 'session-edits-forget.sh' \
    || fail "commit nudge does not mention the forget helper"
  printf '%s' "$out" | grep -q -- "--session $SID" \
    || fail "commit nudge does not interpolate the session id"
}

# Over the signature cap there are no signatures, so nothing says a
# pre-existing dirty path moved at all. Crediting that set is the bead's own
# symptom at a larger scale: a workspace-wide writer command would sweep every
# peer's in-flight file into this session's ledger.
test_over_cap_does_not_credit_pre_existing_dirt() {
  new_repo
  export SESSION_EDITS_MAX_SIG_PATHS=3
  printf 'dirty\n' > "$REPO/src/b.txt"
  printf 'dirty\n' > "$REPO/hud/mod.rs"
  local p; p=$(payload "$SID" "cargo fmt --all")
  run_hook session-edits-bash-pre.sh "$p"
  local i
  for i in 1 2 3 4; do printf 'new\n' > "$REPO/src/n$i.txt"; done
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/b.txt
  assert_not_in_ledger "$SID" hud/mod.rs
  assert_in_suspect "$SID" src/n1.txt
}

# What survives over the cap: the paths the command itself names, out of what
# newly went dirty.
test_over_cap_credits_only_named_paths() {
  new_repo
  export SESSION_EDITS_MAX_SIG_PATHS=3
  printf 'dirty\n' > "$REPO/src/b.txt"
  local p; p=$(payload "$SID" "sed -i '' 's/new/newer/' src/n1.txt")
  run_hook session-edits-bash-pre.sh "$p"
  local i
  for i in 1 2 3 4; do printf 'new\n' > "$REPO/src/n$i.txt"; done
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/n1.txt
  assert_not_in_ledger "$SID" src/n2.txt
  assert_not_in_ledger "$SID" src/b.txt
}

# Commands this repo documents as read-only. A writer-form arm that fires on
# an `install` or `stash` token anywhere in the text hands every peer write in
# their window - minutes long, for the checks.sh line - to this session.
test_documented_read_only_commands_are_not_writer_forms() {
  local c
  for c in "kuluu install list" \
    "kuluu install path horizonxi" \
    "scripts/checks.sh harness comments literals fmt contracts wasm install clippy" \
    "git stash list" \
    "git checkout"; do
    new_repo
    local p; p=$(payload "$SID" "$c")
    run_hook session-edits-bash-pre.sh "$p"
    printf 'peer\n' >> "$REPO/src/a.txt"
    run_hook session-edits-bash-post.sh "$p"
    ledger_has "$SID" src/a.txt && fail "peer write credited during read-only: $c"
    suspect_has "$SID" src/a.txt || fail "peer write not logged as suspect during: $c"
  done
  return 0
}

# `git hash-object --stdin-paths` resolves its input against the worktree root
# whatever -C it is handed, so a session working in a subdirectory signs
# nothing and every candidate reads as unchanged. Pinned on an already-dirty
# path, where the porcelain delta is blind and the signature is the only
# evidence there is - the arm a cwd-relative signature silently disables.
test_attribution_from_a_subdirectory_cwd() {
  new_repo
  printf 'two\n' > "$REPO/src/b.txt"
  local p; p=$(payload_at "$SID" "$REPO/src" "sed -i '' 's/two/three/' b.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'three\n' > "$REPO/src/b.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/b.txt
  assert_not_in_ledger "$SID" b.txt
}

# git_no_hash_shim: hash-object fails and prints nothing, the shape a path that
# vanishes mid-run produces. Everything else reaches the real git.
git_no_hash_shim() {
  mkdir -p "$SANDBOX/bin"
  {
    printf '#!/bin/sh\n'
    printf 'for a in "$@"; do [ "$a" = hash-object ] && exit 128; done\n'
    printf 'exec %s "$@"\n' "$(command -v git)"
  } > "$SANDBOX/bin/git"
  chmod +x "$SANDBOX/bin/git"
  PATH="$SANDBOX/bin:$PATH"
}

# Zero hashes back is reachable, and BSD head rejects `-n 0` outright: the
# pairing must not ask for a zero-length slice, or the hook speaks.
test_zero_signatures_emitted_stays_silent() {
  new_repo
  git_no_hash_shim
  local p; p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
}

# Same guard as the TTL: an operator's junk override reaches `[ -gt ]`, which
# would speak.
test_malformed_max_sig_paths_override_stays_silent() {
  new_repo
  export SESSION_EDITS_MAX_SIG_PATHS=many
  local p; p=$(payload "$SID" "sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

# A command line carries credentials often enough that the log gets the path
# and a reason, never the text that produced it.
test_suspect_log_records_a_reason_not_the_command() {
  new_repo
  local secret="hunter2-do-not-log"
  local p; p=$(payload "$SID" "psql postgres://user:$secret@db.invalid/x -c 'select 1'")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_suspect "$SID" src/a.txt
  local f; f=$(suspect_path "$SID")
  grep -qF "$secret" "$f" && fail "suspect log contains the command text"
  grep -qxF "src/a.txt	$SESSION_EDITS_SUSPECT_UNNAMED" "$f" \
    || fail "suspect log line is not <path><TAB><reason>: $(cat "$f")"
}

# Naming is a path-token test, not a substring one, in both directions.
test_naming_a_nested_path_does_not_credit_its_basename() {
  new_repo
  local p; p=$(payload "$SID" "wc -l hud/mod.rs")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/mod.rs"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" mod.rs
  assert_in_suspect "$SID" mod.rs
}

test_naming_a_basename_does_not_credit_a_nested_path() {
  new_repo
  local p; p=$(payload "$SID" "wc -l mod.rs")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/hud/mod.rs"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" hud/mod.rs
  assert_in_suspect "$SID" hud/mod.rs
}

# A read-only command names its paths to read them: a peer's concurrent write
# to the named path is a suspect, not a ledger line, whatever the content did.
test_read_only_named_path_not_attributed() {
  new_repo
  local p; p=$(payload "$SID" "wc -l src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_suspect "$SID" src/a.txt
}

# git is judged by subcommand: diff reads the path it names, so the peer's
# write to it stays a suspect even though the content genuinely moved.
test_git_diff_named_path_not_attributed() {
  new_repo
  local p; p=$(payload "$SID" "git diff src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_suspect "$SID" src/a.txt
}

# A compound command that also writes is not read-only: the sed -i in the
# second clause keeps the naming arm alive for the path it names.
test_compound_read_only_plus_write_still_attributed() {
  new_repo
  local p; p=$(payload "$SID" "wc -l src/a.txt && sed -i '' 's/alpha/beta/' src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

# bd names no file on its command line, and .beads/issues.jsonl is the export
# that has to cross into git - withholding it silences the commit nudge on a
# file almost every session writes. Crediting stays scoped to the directory bd
# owns, so a peer's concurrent write elsewhere is still withheld.
test_owned_tool_write_is_attributed() {
  new_repo
  local p; p=$(payload "$SID" "bd update kuluu-6mj5 --status in_progress --json")
  run_hook session-edits-bash-pre.sh "$p"
  printf '{"id":"new"}\n' >> "$REPO/.beads/issues.jsonl"
  printf 'peer\n' >> "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" .beads/issues.jsonl
  assert_not_in_ledger "$SID" src/a.txt
  assert_in_suspect "$SID" src/a.txt
}

# The withheld paths need an audience, or a real write that failed the gate
# disappears with no signal at all.
test_commit_nudge_names_the_suspect_log() {
  new_repo
  printf 'edited\n' > "$REPO/src/a.txt"
  ledger_add "$SID" "$REPO" src/a.txt
  suspect_add "$SID" "$REPO" src/b.txt "$SESSION_EDITS_SUSPECT_UNNAMED"
  mkdir -p "$TMPDIR/claude-commit-nudge"
  : > "$TMPDIR/claude-commit-nudge/$SID.porcelain"
  local out rc
  out=$(jq -nc --arg sid "$SID" --arg cwd "$REPO" '{session_id: $sid, cwd: $cwd}' \
    | "$HOOKS/stop.d/20-commit.sh" 2>/dev/null)
  rc=$?
  [ "$rc" = "$FIRE" ] || fail "commit nudge exited $rc, want $FIRE (fire)"
  printf '%s' "$out" | grep -qF "$(suspect_path "$SID")" \
    || fail "commit nudge does not name the suspect log"
}

test_redirect_write_attributed() {
  new_repo
  local p; p=$(payload "$SID" "printf beta > src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  printf 'peer\n' >> "$REPO/src/b.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
  assert_not_in_ledger "$SID" src/b.txt
}

test_absolute_writer_attributed() {
  new_repo
  local p; p=$(payload "$SID" "/usr/bin/python3 gen.py src/a.txt")
  run_hook session-edits-bash-pre.sh "$p"
  printf 'beta\n' > "$REPO/src/a.txt"
  run_hook session-edits-bash-post.sh "$p"
  assert_in_ledger "$SID" src/a.txt
}

test_write_capable_utility_not_readonly() {
  for cmd in 'sort -o src/a.txt src/a.txt' 'uniq src/a.txt src/b.txt' 'xxd -r src/a.txt src/b.txt' 'git diff -- src/b.txt && git restore src/a.txt'; do
    cmd_readonly "$cmd" && fail "write classified as read-only: $cmd"
  done
  return 0
}

CASES=(  test_write_capable_utility_not_readonly
  test_redirect_write_attributed
  test_absolute_writer_attributed
  test_peer_write_not_attributed
  test_peer_write_recorded_as_suspect
  test_named_sed_write_attributed
  test_named_write_to_already_dirty_path_attributed
  test_named_but_unchanged_path_not_attributed
  test_writer_form_without_named_path_attributed
  test_peer_write_outside_fmt_plausible_set_is_suspect
  test_untracked_rs_outside_fmt_plausible_set_is_suspect
  test_rmcm_plausible_set_is_its_operands
  test_redirect_to_dev_null_is_not_a_writer_form
  test_redirect_into_tree_is_a_writer_form
  test_snapshot_key_matches_between_pre_and_post
  test_stale_snapshot_pair_is_swept
  test_stale_ledger_is_swept
  test_live_ledger_survives_the_sweep
  test_malformed_ledger_ttl_override_stays_silent
  test_portable_snapshot_mtime
  test_attribution_survives_gnu_only_stat
  test_snapshot_key_is_single_sourced
  test_malformed_ttl_override_stays_silent
  test_deleted_named_path_is_attributed
  test_ledger_forget_removes_only_named_path
  test_ledger_forget_scoped_to_one_session
  test_commit_nudge_mentions_forget_helper
  test_over_cap_does_not_credit_pre_existing_dirt
  test_over_cap_credits_only_named_paths
  test_documented_read_only_commands_are_not_writer_forms
  test_attribution_from_a_subdirectory_cwd
  test_zero_signatures_emitted_stays_silent
  test_malformed_max_sig_paths_override_stays_silent
  test_suspect_log_records_a_reason_not_the_command
  test_naming_a_nested_path_does_not_credit_its_basename
  test_naming_a_basename_does_not_credit_a_nested_path
  test_read_only_named_path_not_attributed
  test_git_diff_named_path_not_attributed
  test_compound_read_only_plus_write_still_attributed
  test_owned_tool_write_is_attributed
  test_commit_nudge_names_the_suspect_log
)

# Cases share nothing but the read-only template - own sandbox, own TMPDIR,
# own ledger - so they run concurrently and the pre-push gate stays cheap.
# Output is replayed in declaration order afterwards to keep it readable.
RESULTS="$ROOT/results"
mkdir -p "$RESULTS"
for CASE in "${CASES[@]}"; do
  (
    trap - EXIT
    FAILURES=0
    "$CASE" > "$RESULTS/$CASE.out" 2>&1
    [ "$FAILURES" = 0 ] || : > "$RESULTS/$CASE.failed"
    [ "$FAILURES" = 0 ] && printf 'ok   - %s\n' "$CASE" >> "$RESULTS/$CASE.out"
    [ -n "${SANDBOX:-}" ] && rm -rf "$SANDBOX"
    exit 0
  ) &
done
wait

for CASE in "${CASES[@]}"; do
  cat "$RESULTS/$CASE.out" 2>/dev/null || true
  [ -f "$RESULTS/$CASE.failed" ] && FAILURES=$((FAILURES + 1))
done

[ "$FAILURES" = 0 ] || { printf '%s case(s) failed\n' "$FAILURES"; exit 1; }
printf 'all session-edits attribution cases passed\n'
