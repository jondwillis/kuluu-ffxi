#!/usr/bin/env python3
"""beads -> GitHub Issues publisher (one-way projection).

Beads is the durable backlog; its JSONL export is this publisher's snapshot input.
This projects it onto GitHub Issues so contributors have a browsable, linkable
view. Each generated projection maps to one issue, keyed by a hidden marker
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
  scripts/beads-github-publish.py --id ID [--id ID ...] [--include-closed]
                                [--source-export PATH] [--dry-run]
Env:
  REPO                  default jondwillis/kuluu-ffxi
  BEADS_PUBLISH_FILTER  label a bead must carry to be published (default
                        "roadmap"); --all clears it so every bead is published
  DRY_RUN=1             same as --dry-run

Imported external_ref gh-N records link existing issues without rewriting them.
Requires: gh (authenticated), python3. The default source is the checkout export.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlsplit

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
        "maintainer-managed projection of Beads — title/body edits are "
        "overwritten on the next publish. Contributors can discuss here; "
        "maintainers record accepted changes in Beads.\n"
        f"{MARKER.format(id=bid)}"
    )
    parts.append(footer)
    return "\n\n".join(parts)


def load_beads(filter_label: str | None, source_export: Path | None = None) -> list[dict]:
    beads = []
    for line in (source_export or JSONL).read_text().splitlines():
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


def select_beads(beads: list[dict], ids: list[str]) -> list[dict]:
    selected = set(ids)
    counts = {bid: sum(bead["id"] == bid for bead in beads) for bid in selected}
    invalid = {bid: count for bid, count in counts.items() if count != 1}
    if invalid:
        raise ValueError(f"selected IDs must occur exactly once in the export: {invalid}")
    return [bead for bead in beads if bead["id"] in selected]


def is_imported(bead: dict) -> bool:
    return str(bead.get("external_ref") or "").startswith("gh-")


def issue_number_from_url(url: str, repo: str) -> int:
    parsed = urlsplit(url)
    host = os.environ.get("GH_HOST", "github.com")
    match = re.fullmatch(f"/{re.escape(repo)}/issues/([1-9][0-9]*)", parsed.path, re.IGNORECASE)
    if (
        parsed.scheme != "https"
        or parsed.netloc.lower() != host.lower()
        or not match
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(f"issue URL does not match {repo}: {url!r}")
    return int(match.group(1))


def marker_id(body: str) -> str | None:
    start = body.rfind("<!-- beads-id:")
    if start == -1:
        return None
    end = body.find("-->", start)
    if end == -1:
        return None
    return body[start + len("<!-- beads-id:"):end].strip()


def verify_imported_issue(bead: dict, repo: str) -> str:
    reference = bead["external_ref"]
    match = re.fullmatch(r"gh-([1-9][0-9]*)", reference)
    if not match:
        raise ValueError(f"{bead['id']}: invalid imported issue reference {reference!r}")
    number = int(match.group(1))
    issue = json.loads(gh(
        ["issue", "view", str(number), "--repo", repo, "--json", "number,url"],
        capture=True,
    ))
    url = issue.get("url") or ""
    if issue.get("number") != number or issue_number_from_url(url, repo) != number:
        raise ValueError(f"{bead['id']}: imported issue does not match {repo}#{number}: {url}")
    return url


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
        # rfind, not find: project_body appends the real marker last, so a bead
        # whose own description quotes the marker syntax would otherwise index
        # under that literal and get republished as a duplicate every run.
        bid = marker_id(issue.get("body") or "")
        if bid is None:
            continue
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
    ap.add_argument("--id", action="append", default=[], help="publish only this exact bead ID; repeat for multiple IDs")
    ap.add_argument("--source-export", type=Path, default=JSONL, help="read this explicit Beads JSONL snapshot instead of the checkout export")
    ap.add_argument("--include-closed", action="store_true", help="backfill selected closed beads as closed issues; requires --id")
    ap.add_argument(
        "--prune-unmarked",
        action="store_true",
        help="close any OPEN issue that has no beads-id marker (one-time migration off "
        "the legacy ROADMAP-derived issues). Skips hand-filed issues only if they carry "
        "a marker, so use deliberately.",
    )
    ap.add_argument("--dry-run", action="store_true", default=os.environ.get("DRY_RUN") == "1")
    args = ap.parse_args()

    if args.id and (args.all or args.prune_unmarked):
        ap.error("--id cannot be combined with --all or --prune-unmarked")
    if args.include_closed and not args.id:
        ap.error("--include-closed requires --id")
    if not args.source_export.exists():
        print(f"error: {args.source_export} not found", file=sys.stderr)
        return 1

    filter_label = None if args.all or args.id else os.environ.get("BEADS_PUBLISH_FILTER", "roadmap")
    dry = args.dry_run

    protected_imports: set[str] = set()
    try:
        exported = load_beads(None, args.source_export)
        beads = [bead for bead in exported if not filter_label or filter_label in (bead.get("labels") or [])]
        if args.id:
            beads = select_beads(beads, args.id)
        for bead in exported:
            if is_imported(bead):
                match = re.fullmatch(r"gh-([1-9][0-9]*)", bead["external_ref"])
                if not match:
                    if args.prune_unmarked:
                        raise ValueError(f"{bead['id']}: invalid imported issue reference")
                else:
                    protected_imports.add(match.group(1))
    except (ValueError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    scope = f"selected IDs {','.join(sorted(set(args.id)))}" if args.id else (
        "all beads" if args.all else f'beads labelled "{filter_label}"'
    )
    print(f">> repo={args.repo}  source={args.source_export.resolve()}  scope={scope}  count={len(beads)}  DRY_RUN={int(dry)}")

    imported = [bead for bead in beads if is_imported(bead)]
    if args.id:
        try:
            for bead in imported:
                url = verify_imported_issue(bead, args.repo)
                print(f"   reference: {bead['id']} {url} (contributor issue; not modified)")
        except (ValueError, subprocess.CalledProcessError) as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
    elif imported:
        print(f">> skip {len(imported)} imported GitHub references (not outbound projections)")
    beads = [bead for bead in beads if not is_imported(bead)]

    existing = fetch_issues(args.repo)

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
            if bead_closed and not args.include_closed:
                skipped += 1
                continue
            print(f"   create: [{','.join(sorted(want_labels))}] {bid} {title}")
            created_url = gh(
                ["issue", "create", "--repo", args.repo, "--title", title,
                 "--body", body, *(sum((["--label", l] for l in sorted(want_labels)), []))],
                dry=dry,
                capture=True,
            )
            created += 1
            if not dry:
                print(f"   published: {bid} {created_url.strip()}")
            if bead_closed:
                print(f"   close:  {created_url.strip() or '(new issue)'} {bid}")
                if dry:
                    print("+ gh issue close <new issue URL> --repo " + args.repo)
                else:
                    try:
                        number = issue_number_from_url(created_url.strip(), args.repo)
                        confirmed = json.loads(gh(
                            ["issue", "view", str(number), "--repo", args.repo, "--json", "number,url,body"],
                            capture=True,
                        ))
                        if (
                            confirmed.get("number") != number
                            or issue_number_from_url(confirmed.get("url") or "", args.repo) != number
                            or marker_id(confirmed.get("body") or "") != bid
                        ):
                            raise ValueError(f"created issue does not match selected bead {bid}")
                        gh(["issue", "close", str(number), "--repo", args.repo])
                    except (ValueError, subprocess.CalledProcessError) as error:
                        print(f"error: {error}", file=sys.stderr)
                        return 1
                closed += 1
            continue

        num = str(issue["number"])
        if num in protected_imports:
            print(f"   reference: {bid} #{num} (imported issue; not modified)")
            continue
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

    pruned = 0
    if args.prune_unmarked:
        # Read-only list always runs (even in dry); the close respects dry.
        out = gh(
            ["issue", "list", "--repo", args.repo, "--state", "open",
             "--limit", "1000", "--json", "number,body"],
            capture=True,
        )
        for issue in json.loads(out or "[]"):
            if "<!-- beads-id:" in (issue.get("body") or ""):
                continue
            num = str(issue["number"])
            if num in protected_imports:
                continue
            print(f"   prune:  #{num} (no beads-id marker)")
            gh(["issue", "close", num, "--repo", args.repo, "--reason", "not planned"], dry=dry)
            pruned += 1

    print(
        f">> done. created={created} updated={updated} closed={closed} "
        f"reopened={reopened} pruned={pruned} skipped(closed,unpublished)={skipped}"
    )
    if dry:
        print(">> (dry run — GitHub was not modified)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
