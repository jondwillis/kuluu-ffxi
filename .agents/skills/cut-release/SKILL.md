---
name: cut-release
description: >
  Prepare, publish, and verify a Kuluu release through GitHub Actions.
  Use for version bumps, release notes and media, release workflow changes,
  failed publication, missing assets, or updater release discovery failures.
---

# Cut a Kuluu release

Use the existing GitHub Actions pipeline. Prefer `gh` and the GitHub API for release operations.
Use a browser only when the required operation has no suitable API or CLI.

## 1. Establish the release scope

Inspect the working tree, branch, published releases, remote main, and any existing candidate or tag.
Use the previous published version as the changelog base. A prepared candidate tag can make `git describe` select the wrong base.
Review the complete change range, including merged feature branches.
When the user requests all main changes, include both main histories and tracked local edits.
Resolve source ownership before including unrelated local edits.
Use an isolated `release/vX.Y.Z` branch when a release candidate needs separate preparation.

Record the candidate commit, tree, version, previous release, and included branches in the release bead.
Keep structured evidence and temporary drafts under an ignored artifact directory.
For scheduled assessments, stop quietly when the candidate and evidence are unchanged.
Do not put credentials or private fixture files in a public media directory.

## 2. Prepare the candidate

Update `[workspace.package] version` in `Cargo.toml` and run `cargo update --workspace`.
Confirm that the lockfile changes only the intended workspace versions.
If the version is already correct, inspect the lockfile instead of repeating the bump.
Release builds use `--locked`; `cargo metadata --no-deps` does not update the lockfile.

Use `scripts/checks.sh` for applicable checks.
Use the verify skill for affected runtime behavior and inspect its captures.
Keep packaging, gameplay, visual, audio, and original-client comparison results separate.
Preserve completed evidence when a different comparison remains blocked.
Record explicitly deferred work in open follow-up beads. State its exact limits without marking unrelated checks incomplete.

Prepare player-facing notes and inspected media before publication.
For scheduled assessments, present candidate notes, identity, checks, media, and limits before requesting missing publication approval.
Read [notes and media](references/notes-and-media.md) for the API workflow and announcement rules.

## 3. Rehearse the release

Inspect `.github/workflows/release.yml` at the exact candidate commit.
Confirm that the intended trigger supports publication-disabled builds.
On workflows that support it, pushing `release/v*` starts a build-only run.
Reuse that run. Do not dispatch a duplicate rehearsal.
If the automatic trigger is absent, use a supported manual dispatch with `publish=false`.

Keep branch push, build-only dispatch, merge, and publication authorization distinct.
Reuse authorization for the same unchanged candidate and operation.
Before requesting merge approval, explain whether the merge will publish automatically.
Continue authorized local preparation without another approval.

Verify the run's source commit, resolved tag, and publication setting.
Require CI, all six platform packages, bundle assembly, and checksums to pass.
Confirm that the publication job was skipped for a build-only run.
Check executable startup using [platform verification](references/platform-verification.md).
Use native Windows startup checks from the workflow when available; Wine is an additional compatibility surface.

If the merge commit differs, compare trees before reusing source evidence.
Equal trees establish source equivalence. They do not establish identical archive or executable bytes.

## 4. Publish through the pipeline

After applicable approval, merge the release PR or perform the approved main push.
`auto-release-on-main.yml` reads the version, creates an absent tag, and calls `release.yml`.
Follow that run instead of starting another publication.
If publication fails, preserve the diagnostics and read [recovery](references/recovery.md).

Record the final tag commit and workflow run.
Do not change a valid release tag to repair a workflow failure.
Do not interpret a successful build-only run as publication.

## 5. Verify the published result

Use the release API to confirm the requested tag, stable/draft state, and intended latest-release status.
Resolve the tag to the approved commit. Check the asset names against that tag.
Expected packages are macOS ARM64, Linux x64/ARM64, Windows x64/ARM64, and the browser viewer, plus `SHA256SUMS`.

```bash
gh release view vX.Y.Z --json tagName,isDraft,isPrerelease,targetCommitish,assets
gh api repos/jondwillis/kuluu-ffxi/releases/latest
```

Download the published packages and `SHA256SUMS` into an empty evidence directory.
Run `shasum -a 256 -c SHA256SUMS` from that directory.
Checksum entries must use bare asset filenames; the updater consumes those names.

Verify signatures and executable startup for each available compatible runtime.
Retain unsupported architectures as explicit coverage gaps.
If a published archive exactly matches an already checked workflow artifact, reuse that artifact's startup evidence.
For browser evidence reuse, compare the extracted file hashes with the inspected bundle.
HUD startup does not prove live relay gameplay.

Read the published notes again. Verify the public media links and uploaded file hashes.
Complete the final body check described in [notes and media](references/notes-and-media.md).
Close the release bead only after the required publication checks pass.
Report the release URL, results, and remaining claim-specific limits. Keep deferred follow-up beads open.
