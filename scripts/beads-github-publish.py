#!/usr/bin/env python3
"""beads -> GitHub Issues publisher (one-way projection).

beads (`.beads/issues.jsonl`) is the single source of truth for the backlog;
this projects it onto GitHub Issues so contributors have a browsable, linkable
view. Each bead maps to exactly one issue, keyed by a hidden marker
`<!-- beads-id: <id> -->` in the ISSUE BODY (never the title, which we rewrite).

Per run, for each in-scope bead:
  - no matching issue, bead open/in_progress  -> create issue
  - matching issue                            -> patch title/body/managed-labels
                                                 when they drift; reopen/close
                                                 the issue to match bead status
  - no matching issue, bead closed            -> skip (don't backfill finished
                                                 work as a fresh issue)

This is the OUTBOUND half. The inbound half (GitHub issues -> beads) is
scripts/beads-github-sync.sh; the two are independent and not a closed loop, so
don't run them against the same issues expecting a merge.

Idempotent: re-runs only touch issues whose projected content actually changed.
Only labels in the managed namespace (vanilla-parity, enhanced, area:*,
status:*) are added/removed; hand-applied labels (good first issue, …) are left
alone.

Usage:
  scripts/beads-github-publish.py [--repo owner/repo] [--all] [--dry-run]
Env:
  REPO                  default jondwillis/kuluu-ffxi
  BEADS_PUBLISH_FILTER  label a bead must carry to be published (default
                        "roadmap"); --all clears it so every bead is published
  DRY_RUN=1             same as --dry-run

Requires: gh (authenticated), python3. Run where .beads/issues.jsonl lives.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
JSONL = REPO_ROOT / ".beads" / "issues.jsonl"

MARKER = "<!-- beads-id: {id} -->"
MANAGED_PREFIXES = ("area:", "status:")
MANAGED_EXACT = {"vanilla-parity", "enhanced"}

# bead status -> the status:* label projected onto an OPEN issue
STATUS_LABEL = {"open": "status:missing", "in_progress": "status:partial"}

LABEL_COLORS = {
    "vanilla-parity": ("1d76db", "Matches a feature in the official FFXI client"),
    "enhanced": ("5319e7", "Opt-in modernization with no retail analog"),
    "status:missing": ("b60205", "Not started"),
    "status:partial": ("fbca04", "Decoded or scaffolded; UI/dispatch incomplete"),
}


def gh(args: list[str], *, dry: bool = False, capture: bool = False) -> str:
    if dry:
        print("+ gh " + " ".join(args))
        return ""
    res = subprocess.run(
        ["gh", *args], capture_output=capture, text=True, check=True
    )
    return res.stdout if capture else ""


def is_managed(label: str) -> bool:
    return label in MANAGED_EXACT or label.startswith(MANAGED_PREFIXES)


def bead_labels_to_gh(bead: dict) -> set[str]:
    """Map a bead's labels + status onto the GitHub managed-label namespace."""
    out: set[str] = set()
    raw = set(bead.get("labels") or [])
    if "vanilla" in raw:
        out.add("vanilla-parity")
    if "enhanced" in raw:
        out.add("enhanced")
    # Everything that isn't a board/meta tag is treated as an area.
    for label in raw - {"vanilla", "enhanced", "roadmap"}:
        out.add(f"area:{label}")
    if not (out & {"vanilla-parity", "enhanced"}):
        # Enhanced rows in the old roadmap had no area subhead; mirror that.
        pass
    status_label = STATUS_LABEL.get(bead.get("status", "open"))
    if status_label:
        out.add(status_label)
    return out


def project_body(bead: dict, repo: str) -> str:
    parts = [bead.get("description") or "_No description._"]
    ac = bead.get("acceptance_criteria")
    if ac:
        parts.append(f"### Acceptance criteria\n{ac}")
    bid = bead["id"]
    footer = (
        "---\n"
        f"Tracked in beads as **`{bid}`** (`bd show {bid}`). This issue is a "
        "read-only projection of `.beads/issues.jsonl` — edits made here are "
        "overwritten on the next publish; claim and update the work in beads.\n"
        f"{MARKER.format(id=bid)}"
    )
    parts.append(footer)
    return "\n\n".join(parts)


def load_beads(filter_label: str | None) -> list[dict]:
    beads = []
    for line in JSONL.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        bead = json.loads(line)
        if bead.get("_type") and bead["_type"] != "issue":
            continue
        if filter_label and filter_label not in (bead.get("labels") or []):
            continue
        beads.append(bead)
    return beads


def fetch_issues(repo: str) -> dict[str, dict]:
    """Map bead-id -> existing GitHub issue, parsed from the body marker."""
    out = gh(
        [
            "issue", "list", "--repo", repo, "--state", "all", "--limit", "1000",
            "--json", "number,title,body,state,labels",
        ],
        capture=True,
    )
    by_id: dict[str, dict] = {}
    for issue in json.loads(out or "[]"):
        body = issue.get("body") or ""
        i = body.find("<!-- beads-id:")
        if i == -1:
            continue
        j = body.find("-->", i)
        if j == -1:
            continue
        bid = body[i + len("<!-- beads-id:"):j].strip()
        issue["labels"] = [lbl["name"] for lbl in issue.get("labels") or []]
        by_id[bid] = issue
    return by_id


def ensure_labels(repo: str, labels: set[str], dry: bool) -> None:
    for name in sorted(labels):
        color, desc = LABEL_COLORS.get(name, ("0e8a16", f"beads: {name}"))
        try:
            gh(
                ["label", "create", name, "--repo", repo, "--color", color,
                 "--description", desc, "--force"],
                dry=dry,
            )
        except subprocess.CalledProcessError:
            pass  # label already exists / race — harmless


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--repo", default=os.environ.get("REPO", "jondwillis/kuluu-ffxi"))
    ap.add_argument("--all", action="store_true", help="publish every bead, not just the filtered set")
    ap.add_argument("--dry-run", action="store_true", default=os.environ.get("DRY_RUN") == "1")
    args = ap.parse_args()

    if not JSONL.exists():
        print(f"error: {JSONL} not found", file=sys.stderr)
        return 1

    filter_label = None if args.all else os.environ.get("BEADS_PUBLISH_FILTER", "roadmap")
    dry = args.dry_run

    beads = load_beads(filter_label)
    scope = "all beads" if args.all else f'beads labelled "{filter_label}"'
    print(f">> repo={args.repo}  scope={scope}  count={len(beads)}  DRY_RUN={int(dry)}")

    existing = {} if dry else fetch_issues(args.repo)
    if dry:
        print(">> (dry run: skipping the gh issue-list fetch; all beads shown as CREATE)")

    # Pre-create every managed label we'll reference.
    wanted_labels: set[str] = set()
    for bead in beads:
        wanted_labels |= bead_labels_to_gh(bead)
    ensure_labels(args.repo, wanted_labels, dry)

    created = updated = closed = reopened = skipped = 0
    for bead in beads:
        bid = bead["id"]
        title = bead["title"]
        body = project_body(bead, args.repo)
        want_labels = bead_labels_to_gh(bead)
        bead_closed = bead.get("status") == "closed"
        issue = existing.get(bid)

        if issue is None:
            if bead_closed:
                skipped += 1
                continue
            print(f"   create: [{','.join(sorted(want_labels))}] {bid} {title}")
            gh(
                ["issue", "create", "--repo", args.repo, "--title", title,
                 "--body", body, *(sum((["--label", l] for l in sorted(want_labels)), []))],
                dry=dry,
            )
            created += 1
            continue

        num = str(issue["number"])
        cur_managed = {l for l in issue["labels"] if is_managed(l)}
        add = want_labels - cur_managed
        remove = cur_managed - want_labels
        title_changed = issue["title"] != title
        body_changed = (issue.get("body") or "").strip() != body.strip()

        if title_changed or body_changed or add or remove:
            edit = ["issue", "edit", num, "--repo", args.repo]
            if title_changed:
                edit += ["--title", title]
            if body_changed:
                edit += ["--body", body]
            for l in sorted(add):
                edit += ["--add-label", l]
            for l in sorted(remove):
                edit += ["--remove-label", l]
            print(f"   update: #{num} {bid} {title}")
            gh(edit, dry=dry)
            updated += 1

        # Reconcile open/closed state with bead status.
        if bead_closed and issue["state"] == "OPEN":
            print(f"   close:  #{num} {bid}")
            gh(["issue", "close", num, "--repo", args.repo], dry=dry)
            closed += 1
        elif not bead_closed and issue["state"] == "CLOSED":
            print(f"   reopen: #{num} {bid}")
            gh(["issue", "reopen", num, "--repo", args.repo], dry=dry)
            reopened += 1

    print(
        f">> done. created={created} updated={updated} closed={closed} "
        f"reopened={reopened} skipped(closed,unpublished)={skipped}"
    )
    if dry:
        print(">> (dry run — GitHub was not modified)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
