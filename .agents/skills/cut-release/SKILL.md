---
name: cut-release
description: >
  Cut a new Kuluu release and shepherd it to a published, verified GitHub
  release. Use whenever the user asks to make/cut/ship a release, bump the
  version, tag a version, publish a build, "get vX.Y.Z out", or asks why a
  release failed, published under the wrong tag, or is missing assets. Also
  use when editing .github/workflows/release.yml or auto-release-on-main.yml,
  or when the in-app updater stops seeing new versions. Covers the version
  bump, the CI gate, per-platform packaging, publish verification, and the
  recovery path for a bad release.
---

# Cutting a Kuluu release

A release here is **one commit**: bump `[workspace.package] version` in the root
`Cargo.toml`, push it to `main`, and CI does the rest. There is no manual tag
step in the happy path — `auto-release-on-main.yml` reads the workspace version
with `cargo metadata`, sees no matching `v<version>` tag, creates and pushes
one, then calls `release.yml` as a reusable workflow.

The whole job is therefore: **make the bump correct, then verify what CI
actually published.** Green checkmarks are not verification — the failure that
cost the most time on v0.4.0 was a fully green pipeline that published the
release under the tag `main`.

## The happy path

### 1. Decide the version, and check the tree first

Run `git status` before anything. If the tree is dirty, the changes are either
part of this release or they are not — decide *with the user*, don't quietly
sweep them in. In a tree that mixes another session's edits, stage only your own
hunks (`git add -p`); never `git add -A`.

Pick the version from what's actually in the range:

```bash
git log --no-merges --oneline "$(git describe --tags --abbrev=0)"..HEAD
```

New features and protocol/wire changes → minor bump. Fixes only → patch.

### 2. Bump the version *and the lockfile*

```bash
# edit Cargo.toml: [workspace.package] version = "X.Y.Z"
cargo update --workspace
```

`cargo update --workspace` is not optional. Every release leg builds with
`--locked`, so a `Cargo.lock` still carrying the old member versions fails the
build after CI has already spent several minutes. It touches only the workspace
members (13 of them), not the dependency graph — confirm that in the diff.

`cargo metadata --no-deps` does *not* rewrite the lock, so don't reach for it
here even though the workflow uses it to read the version.

### 3. Commit and push

```bash
scripts/checks.sh fmt clippy      # what the pre-push hook runs anyway
git commit -m "chore: release vX.Y.Z"
```

Pushing is outward-facing — confirm with the user before `git push`, per the
repository's standing rule. Check `git log origin/main..HEAD` first: if a
parallel session's commits are riding along, say so explicitly rather than
letting them ship silently.

### 4. Watch the pipeline

```bash
gh run list --limit 5
gh run watch <run-id>
```

The graph is `setup` (resolve tag) → `ci` gate (fmt/clippy/test) → six parallel
build legs (linux / linux-arm / macos / windows / windows-arm / wasm) →
`publish`. The arm legs build natively on GitHub's hosted `ubuntu-24.04-arm`
and `windows-11-arm` runners (free for public repos) — no cross toolchain.

**Rehearse workflow changes before publishing.** Dispatch `release.yml` on the
reviewed branch with `tag=vX.Y.Z-rehearsal` and `publish=false`. An unused tag
is permitted only for rehearsal; an existing tag must resolve to the workflow
commit. The workflow runs CI, builds all platforms, and uploads `release-bundle`
with `SHA256SUMS`; it does not create a tag or release. Download that bundle,
check the manifest, and exercise its binaries. Manual publishing requires
`publish=true` and an existing tag at the workflow commit. The normal main
version-bump caller explicitly opts into publishing. Bind rehearsal evidence
to its workflow SHA and artifact hashes.

**Both Windows legs set `RUSTFLAGS: -C target-feature=+crt-static`.** Rust's
MSVC targets link the C runtime dynamically by default, so without it the exe
imports `VCRUNTIME140.dll` — which Windows does not ship (it comes from the
VC++ redistributable, arch-specific: the x64 redist does not satisfy an ARM64
exe). Ubiquitous on x86_64, routinely absent on fresh Windows-on-ARM, where
v0.5.1 failed at launch (kuluu-ddgn). Verify a Windows release by scanning the
shipped exe for CRT DLL name strings (vcruntime/msvcp/ucrt) — zero hits is the
pass; rehearsal-run artifacts let you check this *before* publishing.

**The windows-arm leg must bypass `rust-toolchain.toml`.** rustup has no
`rustc-codegen-cranelift-preview` dist for `aarch64-pc-windows-msvc`, and the
pinned component list wedges *every* cargo call on that host. The leg installs
the same pinned channel `--profile minimal` and sets `RUSTUP_TOOLCHAIN` (env
beats the file). Cranelift is dev-only; release builds are LLVM. Any future
component added to `rust-toolchain.toml` needs the same check against that
host triple.

**A `cancelled` standalone CI run is benign.** `ci.yml` uses
`concurrency: group: ci-${{ github.ref }}` with `cancel-in-progress: true`, so
the release's own CI gate preempts the push-triggered run on the same ref. The
annotation reads "Canceling since a higher priority waiting request for
ci-refs/heads/main exists". Don't chase it.

### 5. Verify the published release — actually verify it

This is the step that matters. Check four things:

```bash
gh release view vX.Y.Z -R jondwillis/kuluu-ffxi \
  --json tagName,name,isDraft,isPrerelease,isLatest,assets
```

1. **`tagName` is `vX.Y.Z`**, not `main` and not the bare version.
2. **Assets are named for the tag** — `kuluu-vX.Y.Z-aarch64-macos.tar.gz`,
   `-x86_64-linux.tar.gz`, `-aarch64-linux.tar.gz`, `-x86_64-windows.zip`,
   `-aarch64-windows.zip`, `kuluu-viewer-wasm-vX.Y.Z.zip`, plus `SHA256SUMS`.
   If the tag was wrong, the asset names carry the wrong tag too, and the
   in-app updater matches on filename.
3. **Checksums match.** Download and check — the updater verifies against this
   file, so a mismatch bricks in-app updates:
   ```bash
   cd "$(mktemp -d)"
   gh release download vX.Y.Z -R jondwillis/kuluu-ffxi
   shasum -a 256 -c SHA256SUMS
   ```
   (`gh release download` needs `-R` when run outside the repo — it shells out
   to git to infer the repo otherwise.)
4. **The binary launches.** Extract the host-platform archive and run
   `./kuluu --version`. Printing clap usage is a pass — it proves the
   process started. This specifically catches the macOS arm64 trap: `strip`
   invalidates the linker's ad-hoc signature, and an unsigned arm64 binary dies
   with `killed: 9`. `release.yml` re-signs with `codesign --force --sign -`
   after stripping; this check is what confirms it survived.
5. **The aarch64-linux binary launches in colima** (the VM is arm64 Linux).
   The bind mount must live under `/Users` — macOS `mktemp -d` paths aren't
   mounted into the VM — and the bare ubuntu image needs the runtime libs:
   ```bash
   docker run --rm --platform linux/arm64 -v /Users/.../dir:/k ubuntu:24.04 \
     bash -c "apt-get update -q >/dev/null && apt-get install -y -q \
       libwayland-client0 libxkbcommon0 libx11-6 libxcursor1 libxrandr2 \
       libxi6 libasound2t64 libudev1 libdbus-1-3 libgl1 >/dev/null; \
       /k/kuluu --help"
   ```
   The aarch64-windows zip has no local run-proof while the Parallels VMs are
   down; note it as checksum-verified-only in the report.

## When it goes wrong

### The release published under the wrong tag

Symptom: a release named and tagged `main` (or anything non-semver), marked
Latest, with assets named `kuluu-main-*`. Player-visible, because the
in-app updater reads the Latest release plus its `SHA256SUMS`.

Cause, and the general lesson: **a reusable workflow inherits the *caller's*
event context.** On the `workflow_call` path, `github.event_name` is still the
caller's `push` and `GITHUB_REF_NAME` is still `main`. Branching on the event
name to choose between `inputs.tag` and `GITHUB_REF_NAME` therefore silently
picks the branch name. `release.yml`'s `setup` step now prefers `inputs.tag`
unconditionally and hard-fails on a non-`vX.Y.Z` tag, which is why this can't
recur — but the same trap applies to any other `workflow_call` input you add.

This stayed hidden through v0.2.0 and v0.3.0 because those were manually pushed
tags, where `GITHUB_REF_NAME` happened to be right. **The first
version-bump-driven release is where this class of bug surfaces.**

Recovery, in order — all reversible, no history rewriting:

```bash
gh release delete <bad-tag> --yes                # ask the user first: outward-facing
git push origin :refs/tags/<bad-tag>             # delete the remote tag
git push origin :refs/tags/vX.Y.Z                # if a partial/wrong one exists
git tag -d vX.Y.Z                                # local
# commit the workflow fix; pushing to main re-triggers auto-release
```

Then re-verify from step 5. Never force-push and never rewrite shared history to
clean up a release.

### `error: src refspec main matches more than one`

You fetched a stray **local** tag named `main` before the remote one was deleted,
so `main` is ambiguous between the branch and the tag.

```bash
git tag -d main
git push origin refs/heads/main    # fully-qualified ref disambiguates
```

### A build leg fails but CI passed

The legs build with `--locked` and `--features native-window` under the pinned
nightly. The usual causes are the lockfile (step 2) and the per-platform env
overrides in `release.yml` — the macOS `RUSTFLAGS: ""` that drops the
dev-machine lld path, and the `CXXFLAGS: ""` that neutralises the macOS
`-isysroot` for the Recast C++ bridge on Linux/Windows. Those comments in the
workflow explain the *why*; read them before "simplifying" any of them away.

## Things worth knowing

- **`cargo-dist` config exists in `[workspace.metadata.dist]` but is inert.**
  The hand-rolled `release.yml` is what ships. Don't assume dist owns the
  pipeline until the throwaway dry-run tagged `v0.0.0-disttest` has actually
  validated it.
- **`SHA256SUMS` is hashed over bare basenames** (`sha256sum *` from inside the
  artifacts dir) so each line matches the asset filename the updater downloads.
  Adding a directory prefix breaks in-app updates without breaking the release.
- **Windows ships no strip step** — MSVC puts debug info in a separate `.pdb`,
  so the `.exe` is already lean.
- **File the postmortem as a bead.** Anything that went wrong during a release
  is durable work: `bd create --type=bug`. The v0.4.0 tag-resolution bug is
  `kuluu-hevc`.

## Writing the changelog

`generate_release_notes: true` gives GitHub's raw commit list, which is accurate
and unreadable. If the user wants a human changelog, group
`git log --no-merges --pretty='%h %s' vPREV..vNEW` by *player-visible theme* —
what changed in the game — not by crate or by commit order. Lead with the theme
of the release, then sections like combat, rendering, HUD, audio, input. A
reader should learn what's different when they log in, not which files moved.


## Scheduled shadow assessment

Run `python3 scripts/release-preflight.py --state <persistent-ledger.json>`.
Wednesday in America/Los_Angeles selects feature assessment; other days select
hotfix assessment. The script reads current GitHub main and stable identities.
For new inputs, inspect the complete local Git range; GitHub compare can truncate
its file list. Use isolated worktrees and targets. Classify actual changes,
dependencies, gameplay impact and evidence. Select hotfixes from the stable tag
when main contains unfinished features; uncertain dependencies are a hold.
Prepare player-facing notes and an evidence-linked decision card.

Record an assessment JSON with the returned `key`, `decision` (`hold`, `prepared`,
or `no-eligible-fixes`), a nonempty `summary`, evidence paths and coverage gaps.
After completing the assessment, rerun with `--ack <key> --decision <decision>
--evidence <assessment.json>`. The script rejects changed candidate inputs.
Interrupted assessments must not be acknowledged. `no-change` and `unchanged`
permit quiet exits only when tracked blockers and validation inputs are unchanged.
Feed a stable validation-input JSON through `--signals` on both calls so new
evidence reopens holds; do not include timestamps that change every run.

Shadow mode never authorizes publication. Do not push, tag, merge, dispatch a
publishing run or post announcements without applicable user authority. A prepared
candidate still requires approval bound to its commit and artifacts. Missing
runtime, platform or performance evidence is a hold. Archive integrity, example
captures and skipped live tests do not prove gameplay. Major versions and elevated
risk stay behind a human decision.

The offline zone example's `--benchmark-json` emits advisory application-update
intervals, not GPU time or a qualified performance gate. Compare baseline and
candidate on the same host/settings, retain raw samples and inspected captures,
and calibrate noise before setting thresholds. Keep Screen Sharing disconnected
throughout mini runs; display topology changes can terminate the window.
