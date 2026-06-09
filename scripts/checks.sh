#!/usr/bin/env bash
# Single source of truth for the workspace check commands.
#
# Both the local pre-push gate (.githooks/pre-push) and CI
# (.github/workflows/ci.yml) invoke this script instead of spelling out the
# cargo flags themselves, so the two can't drift: a green pre-push run uses
# the *exact* fmt/clippy invocation CI will, and vice versa.
#
# Usage: scripts/checks.sh <stage>...   stage ∈ {fmt, clippy, test, build, doc}
#   scripts/checks.sh fmt clippy              # pre-push default
#   scripts/checks.sh fmt clippy test build   # full CI gate
#
# Each stage is a separate argument so callers (notably CI) can run them as
# distinct steps for per-stage pass/fail reporting while still sharing flags.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# The single feature set the client/viewer build under. Keep in lockstep with
# the release workflow's build flags.
FEATURES=(--features native-window)

run_fmt() {
  cargo fmt --all --check
}

run_clippy() {
  # --all-targets also compiles tests/examples, so stale constructors and
  # broken examples surface as errors here, not just lint warnings. --locked
  # additionally fails on an out-of-date Cargo.lock.
  cargo clippy --workspace --all-targets --locked "${FEATURES[@]}" -- -D warnings
}

run_test() {
  # Integration tests that need a live LSB server self-skip when unreachable,
  # so this is safe on a network-isolated runner.
  #
  # Uses the same --features as clippy/build deliberately: cargo compiles the
  # dependency graph once per feature-set, so matching them lets test reuse the
  # dep artifacts clippy/build already produced instead of recompiling the whole
  # tree under a different feature unification. (No #[test] opens a window — the
  # winit/DefaultPlugins code is confined to examples — so native-window is safe
  # to compile headlessly here.)
  cargo test --workspace --locked "${FEATURES[@]}"
}

run_build() {
  # Same flags the release build uses, so a broken release surfaces early.
  cargo build --workspace --locked "${FEATURES[@]}"
}

run_doc() {
  # Comment/doc-rot discipline. Advisory at the call site (CI marks the step
  # continue-on-error) until the tree reports zero.
  RUSTDOCFLAGS="-W rustdoc::broken_intra_doc_links" \
    cargo doc --workspace --no-deps --document-private-items --locked "${FEATURES[@]}"
  cargo clippy --workspace --locked "${FEATURES[@]}" -- \
    -W clippy::doc_markdown -W clippy::suspicious_doc_comments \
    -W clippy::empty_docs -W clippy::undocumented_unsafe_blocks
}

if [[ $# -eq 0 ]]; then
  echo "checks: no stage given (expected one or more of: fmt clippy test build doc)" >&2
  exit 2
fi

for stage in "$@"; do
  case "$stage" in
    fmt)    echo "checks: fmt";    run_fmt ;;
    clippy) echo "checks: clippy"; run_clippy ;;
    test)   echo "checks: test";   run_test ;;
    build)  echo "checks: build";  run_build ;;
    doc)    echo "checks: doc";    run_doc ;;
    *) echo "checks: unknown stage '$stage'" >&2; exit 2 ;;
  esac
done
