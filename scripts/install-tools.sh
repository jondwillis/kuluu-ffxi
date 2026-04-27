#!/usr/bin/env bash
# Install the project's optional dev CLI tools at pinned versions, so a
# clone / CI / another contributor runs the same tool the hooks suggest.
# Idempotent: skips a tool that is already on PATH at the pinned version.
#
#   rmcm (comment-remover) — bulk comment stripper the Stop comment-rot
#   nudge points at. Pinned to a git commit: crates.io only has 0.1.1,
#   which predates the --diff / -l flags we use; 0.2.1 is git-only.
set -euo pipefail

# comment-remover: repo, pinned commit, and the version that commit builds.
RMCM_GIT="https://github.com/rhythmcache/comment-remover"
RMCM_REV="0c4e5167"
RMCM_VERSION="0.2.1"

if command -v cargo >/dev/null 2>&1; then :; else
  echo "install-tools: cargo not found — install Rust first (https://rustup.rs)" >&2
  exit 1
fi

if command -v rmcm >/dev/null 2>&1 && rmcm --version 2>/dev/null | grep -q "$RMCM_VERSION"; then
  echo "install-tools: rmcm $RMCM_VERSION already installed — skipping"
else
  echo "install-tools: installing rmcm (comment-remover) @ $RMCM_GIT#$RMCM_REV"
  cargo install --git "$RMCM_GIT" --rev "$RMCM_REV" --locked
fi

echo "install-tools: done"
