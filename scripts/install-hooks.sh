#!/usr/bin/env bash
# Point git at the versioned hooks in .githooks/. Run once per clone.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
chmod +x .githooks/* 2>/dev/null || true
echo "installed: core.hooksPath=.githooks (pre-push gate active)"
echo "bypass a push with: git push --no-verify"
