#!/usr/bin/env bash
# Single source of truth for the workspace check commands.
#
# Both the local pre-push gate (.githooks/pre-push) and CI
# (.github/workflows/ci.yml) invoke this script instead of spelling out the
# cargo flags themselves, so the two can't drift: a green pre-push run uses
# the *exact* fmt/clippy invocation CI will, and vice versa.
#
# Usage: scripts/checks.sh <stage>...   stage ∈ {harness, fmt, clippy, test, build, doc}
#   scripts/checks.sh harness fmt clippy      # pre-push default
#   scripts/checks.sh harness fmt clippy test # the CI gate (ci.yml runs these)
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
# the release workflow's build flags (which pass --no-default-features because
# their runners have no DLSS SDK).
#
# dlss is opt-in, but building bevy/dlss needs the DLSS SDK + Vulkan SDK +
# libclang in the environment (dlss_wgpu's build.rs panics without them). When
# this checkout has the in-repo streamline/ SDK and the vars are unset, point
# at it so bare gate runs keep dlss in the lint graph; anywhere else (CI
# runners, release legs, Steam Deck docker) drop default features so kuluu
# builds without dlss.
if [ -z "${DLSS_SDK:-}" ] && [ -d "streamline/sdk/include" ]; then
  export DLSS_SDK="$PWD/streamline/sdk"
fi
if [ -z "${VULKAN_SDK:-}" ] && [ -d "streamline/vulkan-sdk/Include" ]; then
  export VULKAN_SDK="$PWD/streamline/vulkan-sdk"
fi
if [ -z "${LIBCLANG_PATH:-}" ] && [ -d "streamline/llvm/bin" ]; then
  export LIBCLANG_PATH="$PWD/streamline/llvm/bin"
fi
if [ -n "${DLSS_SDK:-}" ] && [ -n "${VULKAN_SDK:-}" ]; then
  FEATURES=(--features native-window,dlss)
else
  echo "checks: no DLSS SDK in the environment — building without the dlss feature"
  FEATURES=(--no-default-features --features native-window)
fi

# Route every cargo invocation through the stall watchdog so a jobserver wedge
# fails loudly instead of hanging the gate — a wedged run here previously sat
# 34 minutes at 0% CPU while holding the build lock (bead kuluu-p5a5). Set
# CARGO_GUARD=0 to bypass (the guard adds ~1s of poll overhead per invocation).
GUARD="$PWD/scripts/cargo-guard.sh"
if [ "${CARGO_GUARD:-1}" = "1" ] && [ -x "$GUARD" ]; then
  cargo() { "$GUARD" "$@"; }
fi

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
  # Style conformance: every HUD file (game windows *and* dev/debug overlays)
  # takes colors/chrome from hud::style (the shared theme). The old
  # hud::palette dev-overlay module is gone; any reference to it — including a
  # reintroduced definition — fails here, which is the point: the unification
  # stays durable as windows are added.
  local hud_dir="kuluu-render/src/hud"
  local bad=()
  for f in "$hud_dir"/*.rs; do
    [[ "$(basename "$f")" == "style.rs" ]] && continue
    if grep -Eq 'hud::palette|palette::|mod palette' "$f"; then
      bad+=("$f")
    fi
  done
  if [[ ${#bad[@]} -gt 0 ]]; then
    echo "checks: style — HUD file(s) reference hud::palette instead of hud::style:" >&2
    printf '  %s\n' "${bad[@]}" >&2
    echo "checks: use hud::style::{theme, text_font, window_frame}; hud::palette was removed" >&2
    return 1
  fi

  # ASCII-only launcher UI: the launcher renders every string with Bevy's
  # bundled default font (FiraMono-subset), which covers only printable ASCII
  # (U+0020-007E) — arrows, em/en dashes, ellipses, middle dots all rasterize
  # as tofu boxes. Gate the whole tree (comments included) so the rule stays a
  # trivial grep with no judgment calls; the same constraint applies by hand to
  # any other string rendered with the default font (e.g. hud::style::text_font).
  if LC_ALL=C grep -rIn '[^ -~]' kuluu/src/view_native/launcher_ui/ --include='*.rs'; then
    echo "checks: style — launcher_ui is not ASCII-only; Bevy's default font renders only U+0020-007E," >&2
    echo "checks:   anything else shows as tofu. Use ASCII substitutes (< > -> ... - | x)." >&2
    return 1
  fi

  # Crate-naming contract: ffxi-* crates are domain truth (facts about the
  # game/LSB, provable against retail) and must not carry Enhanced (non-retail)
  # behavior — that lives in kuluu-* product crates behind opt-in gates. A hit
  # here means an enhanced feature/cfg leaked below the product layer.
  if grep -rIn --include='*.rs' --include='*.toml' 'enhanced' ffxi-*/; then
    echo "checks: style — 'enhanced' found inside an ffxi-* crate; ffxi-* is the faithful" >&2
    echo "checks:   domain layer. Move Enhanced behavior into a kuluu-* crate behind an opt-in." >&2
    return 1
  fi
}

run_harness() {
  # Invariants of the `.agents/` canonical + `.claude/` adapter split
  # (.agents/AGENTS.md holds the mechanism→wiring table this enforces).
  # Pure shell, no cargo — runs first in pre-push because it costs ~nothing.
  # ffxi-agent/ is deliberately out of scope: it ships its own real .claude/
  # tree as the runtime playbook for an agent playing the game.
  local settings=".claude/settings.json" bad=0 link target cmd path doc

  # 1. Every tracked entry under .claude/ is a symlink resolving inside
  #    .agents/, or settings.json itself. Content never lives here.
  while IFS= read -r link; do
    [[ "$link" == "$settings" ]] && continue
    if [[ ! -L "$link" ]]; then
      echo "checks: harness — $link is tracked under .claude/ but is not a symlink" >&2
      echo "checks:   content belongs in .agents/; .claude/ holds symlinks + settings.json" >&2
      bad=1
      continue
    fi
    target=$(cd "$(dirname "$link")" && cd "$(readlink "$(basename "$link")")" 2>/dev/null && pwd) || target=""
    if [[ -z "$target" ]]; then
      echo "checks: harness — $link is a broken symlink (-> $(readlink "$link"))" >&2
      bad=1
    elif [[ "$target" != "$PWD/.agents"* ]]; then
      echo "checks: harness — $link escapes .agents/ (resolves to $target)" >&2
      bad=1
    fi
  done < <(git ls-files .claude)

  # 2. Hooks are path-registered, not directory-discovered — so every command
  #    in settings.json must exist and be executable, and .claude/hooks/ must
  #    stay absent (a reappeared one means someone mirrored the wrong kind).
  if [[ -e ".claude/hooks" ]]; then
    echo "checks: harness — .claude/hooks/ exists; hooks are registered by path in $settings, not discovered by directory" >&2
    bad=1
  fi
  while IFS= read -r cmd; do
    path="${cmd/\$\{CLAUDE_PROJECT_DIR\}\//}"
    [[ "$path" == /* || "$path" == .agents/* || "$path" == scripts/* ]] || continue
    if [[ ! -f "$path" ]]; then
      echo "checks: harness — $settings registers a hook that does not exist: $path" >&2
      bad=1
    elif [[ ! -x "$path" ]]; then
      echo "checks: harness — hook is not executable: $path" >&2
      bad=1
    fi
  done < <(jq -r '.hooks | to_entries[].value[].hooks[]?.command // empty' "$settings" 2>/dev/null)

  # 3. No tracked doc points readers at a root .claude/ path that isn't one the
  #    harness really owns — that is exactly the drift this stage exists to kill
  #    (AGENTS.md long claimed the hooks lived in .claude/hooks/). `~/.claude/…`
  #    is a user-home path, not this adapter, so the regex requires a non-path
  #    char before the dot. .agents/AGENTS.md is exempt: it defines the rule and
  #    must be able to name the paths it forbids; rules 1-2 still cover it.
  while IFS= read -r doc; do
    echo "checks: harness — doc cites an untracked .claude/ path: $doc" >&2
    bad=1
  done < <(git grep -nE '(^|[^/a-zA-Z])\.claude/[a-z]' -- '*.md' \
      ':!ffxi-agent/**' ':!.agents/AGENTS.md' ':!.agents/CLAUDE.md' \
    | grep -vE '\.claude/(settings\.json|settings\.local\.json|skills|agents|worktrees)\b' || true)

  # Cargo records path overrides that no longer match the resolved dependency
  # graph here. Fail before an engine upgrade can silently bypass a required
  # vendor fix while leaving its [patch.crates-io] declaration in place.
  if grep -q '^\[\[patch\.unused\]\]' Cargo.lock; then
    echo "checks: harness - Cargo.lock contains unused [patch.crates-io] overrides:" >&2
    awk '
      /^\[\[patch\.unused\]\]$/ { unused=1; next }
      /^\[\[/ { unused=0 }
      unused && /^(name|version) = / { print "  " $0 }
    ' Cargo.lock >&2
    bad=1
  fi

  return $bad
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
  echo "checks: no stage given (expected one or more of: fmt clippy style harness test build doc)" >&2
  exit 2
fi

for stage in "$@"; do
  case "$stage" in
    fmt)    echo "checks: fmt";    run_fmt ;;
    clippy) echo "checks: clippy"; run_clippy ;;
    style)  echo "checks: style";  run_style ;;
    harness) echo "checks: harness"; run_harness ;;
    test)   echo "checks: test";   run_test ;;
    build)  echo "checks: build";  run_build ;;
    doc)    echo "checks: doc";    run_doc ;;
    *) echo "checks: unknown stage '$stage'" >&2; exit 2 ;;
  esac
done
