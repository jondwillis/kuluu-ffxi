#!/usr/bin/env bash
# Single source of truth for the workspace check commands.
#
# Both the local pre-push gate (.githooks/pre-push) and CI
# (.github/workflows/ci.yml) invoke this script instead of spelling out the
# cargo flags themselves, so the two can't drift: a green pre-push run uses
# the *exact* fmt/clippy invocation CI will, and vice versa.
#
# Usage: scripts/checks.sh <stage>...
#   stage ∈ {harness, comments, fmt, clippy, style, contracts, test, enhanced, build, wasm, doc}
#   scripts/checks.sh harness comments fmt contracts clippy  # pre-push default
#   COMMENTS_DIFF=staged scripts/checks.sh comments  # pre-commit (staged hunks)
#   scripts/checks.sh harness fmt clippy test # the CI gate (ci.yml runs these)
#   scripts/checks.sh enhanced                # the opt-in feature family (CI)
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

# Installing an SDK must not change the vanilla gate's feature graph.
FEATURES=(--no-default-features --features native-window)
if [ "${KULUU_CHECK_DLSS:-0}" = "1" ]; then
  export DLSS_SDK="${DLSS_SDK:-$PWD/vendor/DLSS}"
  if [ ! -f "$DLSS_SDK/include/nvsdk_ngx.h" ] || [ -z "${VULKAN_SDK:-}" ]; then
    echo "checks: DLSS needs its SDK and VULKAN_SDK; see cargo xtask dlss check" >&2
    exit 1
  fi
  FEATURES=(--no-default-features --features native-window,dlss)
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
  # Invariants of the `.agents/` canonical + harness-adapter split
  # (.agents/AGENTS.md holds the mechanism→wiring table this enforces).
  # Pure shell, no cargo — runs first in pre-push because it costs ~nothing.
  # ffxi-agent/ is deliberately out of scope: it ships its own real .claude/
  # tree as the runtime playbook for an agent playing the game.
  local settings=".claude/settings.json" codex_hooks=".codex/hooks.json"
  local codex_config=".codex/config.toml" bad=0 link target cmd path doc
  local hook hook_file recipe check_output

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

  # 3. Beads owns its generated harness adapters. Pin the required shape for
  #    CI, then ask bd itself to detect version drift when it is installed.
  if ! grep -qx 'hooks = true' "$codex_config" 2>/dev/null; then
    echo "checks: harness — $codex_config does not enable native hooks" >&2
    bad=1
  fi
  if ! jq -e '
      ([.hooks.SessionStart[]?.hooks[]? | select(.command == "bd codex-hook SessionStart")] | length == 1) and
      ([.hooks.PreCompact[]?.hooks[]? | select(.command == "bd codex-hook PreCompact")] | length == 1) and
      ([.hooks.PostCompact[]?.hooks[]? | select(.command == "bd codex-hook PostCompact")] | length == 1) and
      ([.hooks.UserPromptSubmit[]?.hooks[]? | select(.command == "bd codex-hook UserPromptSubmit")] | length == 1)
    ' "$codex_hooks" >/dev/null 2>&1; then
    echo "checks: harness — $codex_hooks does not contain one canonical hook per Beads lifecycle event" >&2
    bad=1
  fi
  if ! jq -e '
      [.hooks.SessionStart[]?.hooks[]? | select(.command == "bd prime --hook-json")] | length == 1
    ' "$settings" >/dev/null 2>&1; then
    echo "checks: harness — $settings must register exactly one Beads SessionStart hook" >&2
    bad=1
  fi
  if ! grep -q '<!-- BEGIN BEADS CODEX SETUP:' AGENTS.md \
    || ! grep -q '<!-- BEGIN BEADS INTEGRATION ' AGENTS.md; then
    echo "checks: harness — AGENTS.md is missing a Beads-managed Codex or AGENTS-aware section" >&2
    bad=1
  fi
  if command -v bd >/dev/null 2>&1; then
    for recipe in codex claude factory; do
      if ! check_output=$(bd setup "$recipe" --check 2>&1); then
        echo "checks: harness — stale Beads $recipe integration:" >&2
        echo "$check_output" >&2
        bad=1
      fi
    done
  fi

  # 4. Git has one core.hooksPath, so the versioned project hooks explicitly
  #    dispatch every Beads lifecycle event.
  for hook in pre-commit post-merge pre-push post-checkout prepare-commit-msg; do
    hook_file=".githooks/$hook"
    if [[ ! -x "$hook_file" ]]; then
      echo "checks: harness — missing or non-executable git hook: $hook_file" >&2
      bad=1
    elif ! grep -Fq "bd hooks run $hook" "$hook_file"; then
      echo "checks: harness — $hook_file does not dispatch Beads' $hook lifecycle" >&2
      bad=1
    fi
  done

  # 5. No tracked doc points readers at a root .claude/ path that isn't one the
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

  # 6. Generated bundles stay out of the index. trunk (kuluu-viewer-wasm) and
  #    jacq (ffxi-agent) each write a sibling dist/, and release.yml /
  #    release-wasm.yml always rebuild it from source before zipping — so a
  #    committed copy is stale by construction and costs only clone bandwidth.
  #    GITHUB_MAX_BLOB_BYTES is where GitHub starts warning on push (it hard-
  #    rejects at twice that); the pre-rename wasm bundle here was 58MB.
  local -r GITHUB_MAX_BLOB_BYTES=$((50 * 1024 * 1024))
  local size
  while IFS= read -r path; do
    echo "checks: harness — generated bundle is tracked: $path" >&2
    echo "checks:   dist/ is build output (trunk/jacq); CI rebuilds it and .gitignore covers it" >&2
    bad=1
  done < <(git ls-files -- 'dist/*' '*/dist/*')
  while read -r size path; do
    (( size > GITHUB_MAX_BLOB_BYTES )) || continue
    echo "checks: harness — tracked file over GitHub's $((GITHUB_MAX_BLOB_BYTES / 1024 / 1024))MB blob warning: $path ($size bytes)" >&2
    bad=1
  done < <(git ls-files -s \
    | sed -E -n 's/^(100644|100755|120000) ([0-9a-f]+) [0-3]'$'\t''/\2 /p' \
    | git cat-file --batch-check='%(objectsize) %(rest)')

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

run_comments() {
  # Comment discipline, harness-neutral. The Claude/Codex hooks under
  # .agents/hooks only nudge the agent that registered them; this stage is the
  # gate every contributor, git client, and CI run shares. Hard gates are the
  # families that need no judgment call (a line-pinned citation, a cited path
  # that does not exist in this tree, a finding id with no in-tree record);
  # the heuristic families print as advisory over the lines being added.
  #   COMMENTS_DIFF=staged   scan only the staged hunks (what .githooks/pre-commit runs)
  #   COMMENTS_BASE=<rev>    advisory diff base (default: merge-base with origin/main)
  # shellcheck source=../.agents/hooks/comment-rot.lib.sh
  . .agents/hooks/comment-rot.lib.sh
  local bad=0 lines text
  if [ "${COMMENTS_DIFF:-}" = "staged" ]; then
    lines=$(for f in $(git diff --cached --name-only --diff-filter=AM -- '*.rs'); do
      git diff --cached -U0 -- "$f" | grep -E '^\+[^+]' | sed -E "s#^\+#$f: #"
    done)
  else
    lines=$(git ls-files '*.rs' | grep -vE '^(vendor|research|target|ffxi-agent)/' \
      | xargs grep -nHE '//' 2>/dev/null || true)
  fi
  local comments
  comments=$(printf '%s\n' "$lines" | grep -E '//' | grep -vE 'https?://' || true)

  local hits
  # Match only the comment text: in tree mode each line carries grep's own
  # file:line: prefix, which would otherwise read as a pinned citation.
  hits=$(printf '%s\n' "$comments" | grep -vE "$CR_RE_VERSIONED" | grep -E "//.*$CR_RE_CITE_LINE" || true)
  if [ -n "$hits" ]; then
    echo "checks: comments - citation pinned to a line number; anchor on the symbol (path + function/struct/enumerator):" >&2
    printf '%s\n' "$hits" | cut -c1-200 | sed 's/^/  /' >&2
    bad=1
  fi

  hits=$(printf '%s\n' "$comments" | grep -E "//.*($CR_RE_ELIDED_PATH|$CR_RE_FINDING_ID)" || true)
  if [ -n "$hits" ]; then
    echo "checks: comments - citation nobody can open: an elided .../ path or an out-of-tree finding id. Cite the full in-tree path + symbol:" >&2
    printf '%s\n' "$hits" | cut -c1-200 | sed 's/^/  /' >&2
    bad=1
  fi

  # Every cited in-tree path must exist. vendor/ and research/ roots are only
  # checked when that submodule (or local clone) is populated; docs/ never
  # exists (the tree was retired), so any docs/ citation is dangling.
  local missing=''
  while IFS= read -r tok; do
    [ -z "$tok" ] && continue
    local path root
    path=$(printf '%s' "$tok" | sed -E 's/[.,;:)]+$//; s#/$##')
    case "$path" in
      vendor/game-files*) continue ;;
      vendor/*|research/*)
        root=$(printf '%s' "$path" | cut -d/ -f1-2)
        [ -d "$root" ] && [ -n "$(ls -A "$root" 2>/dev/null)" ] || continue ;;
    esac
    case "$path" in *..*) continue ;; esac
    # A path wrapped at a line break, or one with spaces in it, reaches here
    # truncated, so a prefix match of a real entry is accepted too. Outside
    # the submodule roots the entry has to be tracked: an untracked local
    # note passes a filesystem test on its author's machine and nowhere else.
    case "$path" in
      vendor/*|research/*) [ -n "$(ls -d "$path"* 2>/dev/null)" ] && continue ;;
      *) [ -n "$(git ls-files -- "$path*" 2>/dev/null)" ] && continue ;;
    esac
    missing+="  $path"$'\n'
  done < <(printf '%s\n' "$comments" \
    | grep -oE '(^|[^A-Za-z0-9._/-])(vendor|research|docs|artifacts|\.agents)/[A-Za-z0-9._/-]+' \
    | sed -E 's#^[^A-Za-z0-9._/-]##' | sort -u)
  if [ -n "$missing" ]; then
    echo "checks: comments - cited path does not exist in this tree (moved upstream, a private note, or the retired docs/ tree); fix or drop the citation:" >&2
    printf '%s' "$missing" >&2
    bad=1
  fi

  # Advisory: the judgment-call families over the lines being added. Never fails.
  if [ "${COMMENTS_DIFF:-}" = "staged" ]; then
    text=$(printf '%s\n' "$lines" | sed -E 's/^[^:]+: //')
  else
    local base
    base=${COMMENTS_BASE:-$(git merge-base HEAD origin/main 2>/dev/null || true)}
    text=''
    [ -n "$base" ] && text=$(git diff -U0 "$base" -- '*.rs' | grep -E '^\+[^+]' | sed -E 's/^\+//' || true)
  fi
  if [ -n "$text" ]; then
    local findings
    findings=$( { printf '%s\n' "$text" | scan_comment_rot || true; \
                  printf '%s\n' "$text" | scan_code_magic || true; } | grep -v '^[[:space:]]*$' || true)
    if [ -n "$findings" ]; then
      echo "checks: comments (advisory) - new comments/literals to judge before this lands:"
      printf '%s\n' "$findings"
      echo "checks:   keep a comment only for a WHY you cannot encode, a vendor/spec citation, or a SAFETY note; name a literal as a const."
    fi
  fi
  return $bad
}

run_contracts() {
  local contract="session::event_transport::contracts::event_state_contract" listing
  listing=$(cargo test -p kuluu-session --lib --locked -- --list)
  if ! grep -Fxq "$contract: test" <<< "$listing"; then
    echo "checks: contracts — mandatory event state contract is missing" >&2
    return 1
  fi
  cargo test -p kuluu-session --lib --locked "$contract" -- --exact --include-ignored
  listing=$(cargo test -p kuluu-render -p kuluu --lib --locked "${FEATURES[@]}" -- --list)
  for contract in \
    transport::tests::transport_state_contract \
    view_native::input::tests::scripted_walk_render_contract \
    view_native::walker::obstacles::tests::transport_dock_collision_contract \
    view_native::navmesh_overlay::tests::remote_passenger_keeps_reported_height_under_unloaded_interior_shell \
    zone_point_lights::tests::active_interior_lights_join_main_and_leave_on_deactivation_or_disconnect; do
    if ! grep -Fxq "$contract: test" <<< "$listing"; then
      echo "checks: contracts — mandatory transport render contract is missing: $contract" >&2
      return 1
    fi
    cargo test -p kuluu-render -p kuluu --lib --locked "${FEATURES[@]}" "$contract" -- --exact --include-ignored
  done
  listing=$(cargo test -p ffxi-dat --lib --locked -- --list)
  for contract in \
    mmb::tests::legacy_mmb_static_canopy_preserves_strip_connectivity_and_winding \
    vehicle::tests::nonuniform_spline_matches_retail_weighted_basis_and_endpoint_extension; do
    if ! grep -Fxq "$contract: test" <<< "$listing"; then
      echo "checks: contracts — mandatory vehicle DAT contract is missing: $contract" >&2
      return 1
    fi
    cargo test -p ffxi-dat --lib --locked "$contract" -- --exact --include-ignored
  done
}

run_test() {
  run_contracts
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

run_enhanced() {
  # The Enhanced (non-retail) family is opt-in, so FEATURES above — the vanilla
  # gate every other stage runs — never even type-checks it, and code under
  # `#[cfg(feature = "enhanced-…")]` (with its tests) rots unseen. This stage is
  # that family's compile+test leg. The list is read out of kuluu's manifest so
  # a newly declared feature is covered without editing a second list here;
  # enhanced-neural-uplift is the one exclusion, because it implies dlss and so
  # needs the SDK that KULUU_CHECK_DLSS gates.
  local enhanced
  enhanced=$(grep -oE '^enhanced-[a-z-]+' kuluu/Cargo.toml \
    | grep -v '^enhanced-neural-uplift$' | paste -sd, - || true)
  if [[ -z "$enhanced" ]]; then
    echo "checks: enhanced — no enhanced-* features found in kuluu/Cargo.toml" >&2
    return 1
  fi
  local features=(--no-default-features --features "native-window,$enhanced")
  cargo clippy --workspace --all-targets --locked "${features[@]}" -- -D warnings
  cargo test --workspace --locked "${features[@]}"
}

run_build() {
  # Local-only convenience: a dev-profile, non-test compile+link of the whole
  # workspace. CI does NOT run this — `cargo test` already compiles and links
  # every lib/bin (so it is the CI compile gate), and release.yml builds the
  # real per-OS --release artifacts. This is a fast local proxy for the latter,
  # but note it is dev-profile/Cranelift, not the release LLVM build.
  cargo build --workspace --locked "${FEATURES[@]}"
}

run_wasm() {
  cargo check -p kuluu-viewer-wasm --locked --target wasm32-unknown-unknown
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
  echo "checks: no stage given (expected one or more of: fmt clippy style harness contracts test enhanced build wasm doc)" >&2
  exit 2
fi

for stage in "$@"; do
  case "$stage" in
    fmt)    echo "checks: fmt";    run_fmt ;;
    clippy) echo "checks: clippy"; run_clippy ;;
    style)  echo "checks: style";  run_style ;;
    comments) echo "checks: comments"; run_comments ;;
    harness) echo "checks: harness"; run_harness ;;
    contracts) echo "checks: contracts"; run_contracts ;;
    test)   echo "checks: test";   run_test ;;
    enhanced) echo "checks: enhanced"; run_enhanced ;;
    build)  echo "checks: build";  run_build ;;
    wasm)   echo "checks: wasm";   run_wasm ;;
    doc)    echo "checks: doc";    run_doc ;;
    *) echo "checks: unknown stage '$stage'" >&2; exit 2 ;;
  esac
done
