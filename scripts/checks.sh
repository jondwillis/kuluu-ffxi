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
#   scripts/checks.sh fmt clippy test         # the CI gate (ci.yml runs these)
#   scripts/checks.sh build                   # local-only: see run_build below
#
# Each stage is a separate argument so callers (notably CI) can run them as
# distinct steps for per-stage pass/fail reporting while still sharing flags.
set -euo pipefail

# GUI git clients (Fork, Tower, GitKraken…) run hooks with a stripped PATH that
# omits ~/.cargo/bin, so `cargo` isn't found. Pull in rustup's env when it's
# missing. No-op on CI / interactive shells, where cargo is already on PATH.
if ! command -v cargo >/dev/null 2>&1; then
  if [ -f "$HOME/.cargo/env" ]; then
    . "$HOME/.cargo/env"
  else
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
fi

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
  run_style
}

run_style() {
  # Game-window style conformance: player-facing HUD must take colors/chrome
  # from hud::style (the shared game theme), never hud::palette (dev-overlay
  # colors). This allowlist is the *only* set of files that may reference the
  # palette; a new HUD file that reaches for it fails here, which is the point
  # — the unification stays durable as windows are added.
  local allow=(
    mod.rs           # defines the palette
    diagnostics.rs
    mesh_debug.rs
    network_status.rs
    overlay.rs
    stage_bar.rs
  )
  local hud_dir="ffxi-viewer-core/src/hud"
  local bad=()
  for f in "$hud_dir"/*.rs; do
    local base
    base="$(basename "$f")"
    local allowed=0
    for a in "${allow[@]}"; do
      [[ "$base" == "$a" ]] && allowed=1 && break
    done
    [[ $allowed -eq 1 ]] && continue
    if grep -Eq 'hud::palette|palette::' "$f"; then
      bad+=("$f")
    fi
  done
  if [[ ${#bad[@]} -gt 0 ]]; then
    echo "checks: style — game-window file(s) reference hud::palette instead of hud::style:" >&2
    printf '  %s\n' "${bad[@]}" >&2
    echo "checks: use hud::style::{theme, text_font, window_frame}; palette is dev-overlay-only" >&2
    return 1
  fi
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
  # Local-only convenience: a dev-profile, non-test compile+link of the whole
  # workspace. CI does NOT run this — `cargo test` already compiles and links
  # every lib/bin (so it is the CI compile gate), and release.yml builds the
  # real per-OS --release artifacts. This is a fast local proxy for the latter,
  # but note it is dev-profile/Cranelift, not the release LLVM build.
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

  # Architecture-map drift: every workspace crate must be named in AGENTS.md
  # so a newly added crate can't slip in undocumented. Advisory like the rest
  # of this stage — warns, never fails.
  local missing=()
  for crate in $(grep -oE '"ffxi-[a-z-]+"' Cargo.toml | tr -d '"' | sort -u); do
    grep -q "$crate" AGENTS.md || missing+=("$crate")
  done
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "checks: doc-drift — crate(s) absent from AGENTS.md: ${missing[*]}" >&2
  fi
}

if [[ $# -eq 0 ]]; then
  echo "checks: no stage given (expected one or more of: fmt clippy style test build doc)" >&2
  exit 2
fi

for stage in "$@"; do
  case "$stage" in
    fmt)    echo "checks: fmt";    run_fmt ;;
    clippy) echo "checks: clippy"; run_clippy ;;
    style)  echo "checks: style";  run_style ;;
    test)   echo "checks: test";   run_test ;;
    build)  echo "checks: build";  run_build ;;
    doc)    echo "checks: doc";    run_doc ;;
    *) echo "checks: unknown stage '$stage'" >&2; exit 2 ;;
  esac
done
