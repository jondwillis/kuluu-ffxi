---
name: beads-github-sync
description: How beads issues are projected to GitHub Issues and (optionally) imported back. Use when publishing beads to GitHub, debugging why a bead's issue is stale or missing, editing scripts/beads-github-publish.py or scripts/beads-github-sync.sh, or touching .github/workflows/beads-github-publish.yml.
---

# Beads ↔ GitHub Issues

Beads (`.beads/`) is the durable project tracker. Issues carrying a `<!-- beads-id: ID -->` marker are generated projections; maintainers must record decisions from their GitHub discussion in Beads. Ordinary contributor-filed issues retain their own title/body and are not generated projections.

## Outbound: beads → GitHub

`scripts/beads-github-publish.py` is the publisher. Each in-scope bead maps to one issue keyed by a `<!-- beads-id: <id> -->` body marker. It keeps in sync:

- title and body
- managed labels: `vanilla-parity` / `enhanced` / `area:*` / `status:*`
- open/closed state — closing a bead closes its issue

`.github/workflows/beads-github-publish.yml` runs it automatically on every push to `main` that touches `.beads/issues.jsonl`, publishing **all** beads (not just `roadmap`-labelled ones). `workflow_dispatch` remains available for manual runs, where `dry_run` defaults to true.

Dry runs need authenticated GitHub read access to compare existing projections and preview creates, updates, closures and reopenings accurately. They print proposed writes without executing them; offline regression tests guard that boundary before the workflow publishes.

**Automatic main-branch publication requires the exported `.beads/issues.jsonl` to be committed and pushed.** Local edits do not trigger that workflow. The scoped path below can publish a reviewed local snapshot before its PR lands.

### Publish PR-linked tasks before review

Use a trusted checkout of the publisher and select only the exact Beads IDs referenced by the PR. Read those records and the proposed issue bodies before publication. Do not run a bulk reconciliation to fill a few missing links.

The default `--source-export` is the publisher checkout's `.beads/issues.jsonl`; an isolated or old worktree can contain an outdated export. To publish current local records, export the authoritative Beads DB to a temporary snapshot without staging a mixed export:

```bash
bd where
bd export --output /private/tmp/kuluu-pr-issues.jsonl
shasum -a 256 /private/tmp/kuluu-pr-issues.jsonl
python3 scripts/beads-github-publish.py --repo jondwillis/kuluu-ffxi \
  --source-export /private/tmp/kuluu-pr-issues.jsonl \
  --id BEAD_ID --id OTHER_BEAD_ID --include-closed --dry-run
```

Replace the placeholder IDs with the verified PR-linked tasks. Record the DB identity, snapshot hash and publisher revision in the review evidence. Use that same snapshot and selector for the authorized publication, removing only `--dry-run`; confirm its hash still matches. Then read the resulting issues and use their actual URLs in the PR. A snapshot publication does not commit or synchronize the DB/export; maintainers still commit the relevant export records through the normal main-branch workflow.

`--id` bypasses the roadmap filter, rejects missing or duplicate records before any GitHub call, and cannot accompany `--all` or `--prune-unmarked`. Repeated IDs select a record once. `--include-closed` requires IDs and creates then closes an unpublished selected finished task; omit it when that history is unnecessary. Ordinary bulk publication still skips unpublished closed tasks.

An imported `external_ref: gh-N` is an existing contributor issue, not a fresh projection. Scoped publication verifies its number and repository URL with an authenticated read, reports the link and leaves its body, labels and state intact. Bulk runs skip imported references. A failed reference check stops the run before any writes, including other selected tasks. Resolve wrong-repository or stale references in Beads instead of creating duplicates.

Closed backfill confirms the created issue URL and reads its Beads marker before closing that numeric issue in the selected repository. If creation succeeds but closure fails, rerun the same scoped command; the marker reuses the issue instead of creating another. The legacy `--prune-unmarked` migration also protects issue numbers referenced by imported records anywhere in the snapshot; never combine pruning with PR-linked publication.

Dry runs require issue read access. Actual projection creation/edit/close and managed-label creation require an authenticated maintainer with the repository's corresponding issue and label permissions, or the existing main-branch workflow token (`issues: write`). Do not hand this credential to a contributor or add a privileged workflow that executes fork content. Keep the automatic main trigger and its bulk reconciliation unchanged.

### Contributors without repository membership

Contributors can [create an ordinary GitHub issue with read access when issues are enabled](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue), comment on an existing issue, or submit a fork PR linking it. They do not need Beads, permission to edit other people's issues, labels, or an administrative token. A maintainer adopts the work into the durable tracker and verifies the issue reference. For a projected issue, contributor comments remain the discussion surface; the maintainer mirrors accepted updates into Beads. Do not ask a nonmember to run the publisher/importer or edit a generated issue body.

## Inbound: GitHub → beads

`scripts/beads-github-sync.sh` is independent and opt-in. It imports GitHub issues *into* beads, keyed on `external_ref: gh-<number>`.

Do not bulk-import generated projections: the importer keys by `gh-N`, rather than their existing Beads marker, and can create duplicate tracker records. To adopt an ordinary contributor issue, first read its title/body and verify its repository URL, then create one durable task with `bd create "Issue title" --external-ref gh-N --description "Reviewed issue details"`; replace `N` with the actual issue number and supply the appropriate acceptance/design fields. Check for an existing matching external reference before creation. This is a maintainer operation, not a contributor requirement.

Imported issue state is not automatically reconciled by the outbound publisher. Maintainers record accepted decisions and completion in Beads and separately close/reopen the original GitHub issue through normal issue triage, using the corresponding repository permissions. Linking a record does not turn its contributor-owned body into a generated projection.

The two directions are **not** a loop. Imported `gh-N` records are excluded from outbound rewriting, even when their GitHub issue happens to contain a projection marker. The importer does not grant publication authority or prove a same-number issue belongs to this repository; verify that mapping before review.
