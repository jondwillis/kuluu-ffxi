# Release workflow recovery

## Resolve identity before retrying

Inspect the failed job, resolved inputs, event context, tag commit, and candidate commit.
Distinguish a source failure, packaging failure, publication failure, and missing authorization.
Preserve the failed run and diagnostics in the release bead.
Do not restart successful jobs merely because a different check failed.

A reusable workflow inherits its caller's event context.
For a call from main, `GITHUB_EVENT_NAME` can remain `push` and `GITHUB_REF_NAME` can remain `main`.
An explicit tag input must take precedence over that inherited branch name.
Restrict automatic build-only branch handling to `release/v*`; do not apply it to every branch push.

When changing trigger logic, run `python3 scripts/release-candidate.test.py`.
Exercise inherited main calls, release-branch pushes, tag pushes, and manual dispatch with both publication settings.
Prove the reported regression fails with the old condition.
Keep the tests in the shared harness gate.

## Retry an unchanged approved release

If the existing tag points to the correct commit, preserve it.
If the tagged workflow supports a working manual path, use that path for recovery:

```bash
gh workflow run release.yml --ref vX.Y.Z -f tag=vX.Y.Z -f publish=true
```

Use this only within the approved publication scope.
Confirm the run's source SHA, resolved tag, and publication setting before accepting its result.
Make one supported retry after resolving the cause. If the same failure returns, stop and report the diagnostic.
Track a permanent workflow correction separately. Do not silently merge that correction under release publication authority.
An unchanged main push can skip publication when the version tag already exists; inspect the workflow before relying on it.

If the tag or published assets are wrong, prepare a specific recovery plan.
Obtain applicable approval before deletion or tag replacement. Never force-push or rewrite shared history as routine recovery.
Recheck all published assets after recovery.

## Packaging failures

- macOS: stripping invalidates ARM64 signatures. Re-sign the executable and verify both executable and app bundle signatures.
- Windows: both architectures need static CRT linkage. Inspect imports for `vcruntime`, `msvcp`, and `ucrt`; none should remain.
- Windows ARM64: install the pinned nightly without unavailable Cranelift components. Use `RUSTUP_TOOLCHAIN` to override the repository component list.
- Linux and Windows: inspect platform environment overrides before changing compiler or SDK flags.
- Lockfile: every release leg uses `--locked`. A version-only manifest edit can leave workspace package versions stale.

Use `.github/workflows/release.yml` and its packaging scripts as the current command source.
Existing `cargo-dist` metadata does not establish that cargo-dist owns publication.

A standalone CI run can be cancelled when the release's CI run uses the same concurrency group.
Confirm that replacement before treating cancellation as a failure.
