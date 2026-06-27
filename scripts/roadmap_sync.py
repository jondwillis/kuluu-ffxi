#!/usr/bin/env python3
"""Reconcile docs/ROADMAP.md glyphs against the roadmap-labelled beads.

Beads are the system of record for parity status; the ROADMAP scoreboard is a
human-readable view of it. This keeps the glyph column honest without throwing
away the curated prose:

  bead status -> glyph
    closed       [x]   (unambiguous: the feature is done)
    in_progress  [~]   (partial / scaffolded)
    open         [ ]   (not started)

Matching: an explicit `<!-- bead:kuluu-xxx -->` trailing anchor wins; otherwise
a line is matched to a bead by normalized title prefix. Unmatched lines/beads
are reported, never guessed.

Modes:
  --check   report drift; exit 1 if any *closed* bead's line isn't [x], or any
            [x] line's bead isn't closed (the unambiguous, CI-blockable class).
            open/in_progress vs [ ]/[~] disagreements print as warnings only.
  --write   apply the safe transform (closed bead -> [x]); leave the ambiguous
            [ ]/[~] distinction for a human. Prints every change.

Reads .beads/issues.jsonl (the committed export) directly — no `bd`/Dolt
needed, so it runs deterministically in CI.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROADMAP = ROOT / "docs" / "ROADMAP.md"
ISSUES_JSONL = ROOT / ".beads" / "issues.jsonl"
GLYPH_RE = re.compile(r"^(?P<indent>\s*)- \[(?P<glyph>[ x~?])\] (?P<text>.*)$")
ANCHOR_RE = re.compile(r"<!--\s*bead:(?P<id>[a-z0-9-]+)\s*-->")
STATUS_GLYPH = {"closed": "x", "in_progress": "~", "open": " "}


def norm(title: str) -> str:
    head = re.split(r"[—:]| - ", title, maxsplit=1)[0]
    return re.sub(r"[^a-z0-9]", "", head.lower())[:28]


def load_beads() -> list[dict]:
    """Read the committed beads export (the file that crosses git), not the
    local Dolt db — so the check is deterministic and needs no `bd` in CI."""
    beads = []
    for line in ISSUES_JSONL.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        b = json.loads(line)
        if "roadmap" in (b.get("labels") or []):
            beads.append(b)
    return beads


def build_index(beads: list[dict]) -> tuple[dict, dict]:
    by_id = {b["id"]: b for b in beads}
    by_norm: dict[str, dict] = {}
    for b in beads:
        by_norm.setdefault(norm(b["title"]), b)
    return by_id, by_norm


def match(text: str, by_id: dict, by_norm: dict) -> dict | None:
    anchor = ANCHOR_RE.search(text)
    if anchor:
        return by_id.get(anchor.group("id"))
    key = norm(text)
    if len(key) < 6:
        return None
    return by_norm.get(key)


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "--check"
    if mode not in ("--check", "--write"):
        print(f"usage: {sys.argv[0]} [--check|--write]", file=sys.stderr)
        return 2

    beads = load_beads()
    by_id, by_norm = build_index(beads)
    lines = ROADMAP.read_text().splitlines()

    hard_drift: list[str] = []   # closed-bead vs non-[x], or [x] vs non-closed
    soft_drift: list[str] = []   # [ ] vs [~] (open vs in_progress) disagreements
    matched_ids: set[str] = set()
    changes = 0

    for i, line in enumerate(lines):
        m = GLYPH_RE.match(line)
        if not m:
            continue
        bead = match(m.group("text"), by_id, by_norm)
        if not bead:
            continue
        matched_ids.add(bead["id"])
        cur = m.group("glyph")
        want = STATUS_GLYPH.get(bead["status"])
        if want is None or cur == want:
            continue
        tag = f"{bead['id']} {bead['status']:<11} [{cur}]->[{want}] {bead['title'][:48]}"
        is_hard = cur == "x" or want == "x"
        if is_hard:
            hard_drift.append(tag)
            # Only auto-PROMOTE to [x] from a closed bead (unambiguous). An
            # [x] line whose bead isn't closed is over-claiming OR a stale
            # bead — leave it for a human, never silently downgrade.
            if mode == "--write" and want == "x":
                lines[i] = f"{m.group('indent')}- [{want}] {m.group('text')}"
                changes += 1
        else:
            soft_drift.append(tag)

    unmatched_beads = [
        f"{b['id']} {b['status']:<11} {b['title'][:56]}"
        for b in beads
        if b["id"] not in matched_ids and b["status"] != "closed"
    ]

    if mode == "--write" and changes:
        ROADMAP.write_text("\n".join(lines) + "\n")

    print(f"beads={len(beads)} matched={len(matched_ids)} "
          f"hard_drift={len(hard_drift)} soft_drift={len(soft_drift)} "
          f"unmatched_open_beads={len(unmatched_beads)}")
    for label, rows in (("HARD DRIFT (closure mismatch)", hard_drift),
                        ("SOFT DRIFT ([ ] vs [~], review)", soft_drift),
                        ("UNMATCHED OPEN BEADS (no roadmap line)", unmatched_beads)):
        if rows:
            print(f"\n{label}:")
            for r in rows:
                print(f"  {r}")

    if mode == "--write":
        print(f"\nwrote {changes} glyph change(s) to {ROADMAP.name}" if changes
              else "\nno safe glyph changes to write")
        return 0
    return 1 if hard_drift else 0


if __name__ == "__main__":
    sys.exit(main())
