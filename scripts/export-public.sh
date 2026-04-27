#!/usr/bin/env bash
#
# export-public.sh — publish this repo to its public home as a SINGLE commit,
# without rewriting this working repo's history or disturbing its worktrees.
#
# How it works: builds a throwaway orphan branch from the CURRENT committed
# tree (one root commit, no history), pushes it to the public remote's main,
# then deletes the orphan branch and returns you to where you were. `main` and
# any active worktrees are never touched.
#
# Run this once the working tree is SETTLED and committed (e.g. after any
# concurrent workflow finishes and you've committed the publishing prep).
#
# Usage:
#   scripts/export-public.sh                      # -> git@github.com:jondwillis/kuluu-ffxi.git
#   PUBLIC_REMOTE=git@github.com:you/repo.git scripts/export-public.sh
#   MSG="Initial public release" scripts/export-public.sh
#   FORCE=1 scripts/export-public.sh              # force-push (re-publishing)
#
# Requires: git, a clean working tree, and the public repo to already exist
# (create it empty on GitHub first).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PUBLIC_REMOTE="${PUBLIC_REMOTE:-git@github.com:jondwillis/kuluu-ffxi.git}"
MSG="${MSG:-Initial public release: Kuluu — an open-source FINAL FANTASY XI client in Rust + Bevy}"
ORPHAN="_publish_$(git rev-parse --short HEAD)"
FORCE="${FORCE:-0}"

# 1. Refuse on a dirty tree — we publish a settled, committed snapshot only.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "error: working tree is dirty. Commit (or stash) everything first," >&2
  echo "       so the published commit is a coherent snapshot." >&2
  git status --short >&2
  exit 1
fi

START_REF="$(git symbolic-ref --quiet --short HEAD || git rev-parse HEAD)"
echo ">> publishing $(git rev-parse --short HEAD) ($START_REF) as a single commit to:"
echo "   $PUBLIC_REMOTE  (main)"

# Always return to where we started, even on error.
cleanup() {
  git checkout --quiet "$START_REF" 2>/dev/null || true
  git branch -D "$ORPHAN" 2>/dev/null || true
}
trap cleanup EXIT

# 2. Orphan branch = current tree, zero history.
git checkout --quiet --orphan "$ORPHAN"
git add -A
git commit --quiet -m "$MSG"
echo ">> built single-commit tree $(git rev-parse --short HEAD)"

# 3. Push to the public remote's main.
push_args=(--quiet "$PUBLIC_REMOTE" "HEAD:refs/heads/main")
[ "$FORCE" = "1" ] && push_args=(--force "${push_args[@]}")
echo ">> pushing… (FORCE=$FORCE)"
git push "${push_args[@]}"

echo ">> done. https://${PUBLIC_REMOTE#git@github.com:}" | sed 's/\.git$//'
echo ">> your working repo is untouched; back on '$START_REF'."
# cleanup() runs on EXIT
