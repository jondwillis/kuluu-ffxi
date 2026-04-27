#!/usr/bin/env python3
"""Emit beads-import JSONL from the open [ ]/[~] lines of docs/ROADMAP.md.

Each open (missing) or partial line becomes one beads issue tagged with its
ROADMAP section and scoreboard, so the parity board and the tracker stay
legible together. A stable external_ref (roadmap:<section>:<slug>) lets bd's
import upsert keep re-runs idempotent rather than duplicating.

    python3 scripts/beads-seed-from-roadmap.py | bd import -
"""
import json
import re
import sys
from pathlib import Path

ROADMAP = Path(__file__).resolve().parents[1] / "docs" / "ROADMAP.md"

CHECK = re.compile(r"^(?P<indent>\s*)- \[(?P<glyph>[ x~?])\] (?P<text>.*)$")
SECTION = re.compile(r"^###\s+(?P<name>.+?)\s*$")
SCOREBOARD = re.compile(r"^##\s+(?P<name>.+?)\s*$")


def slugify(s: str) -> str:
    s = re.sub(r"`[^`]*`", "", s)
    s = re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
    return s[:48] or "item"


def parse(lines):
    section, scoreboard, cur = "general", "vanilla", None
    items = []
    for raw in lines:
        if m := SCOREBOARD.match(raw):
            scoreboard = "enhanced" if "enhanced" in m.group("name").lower() else "vanilla"
            cur = None
        elif m := SECTION.match(raw):
            section, cur = slugify(m.group("name")), None
        elif m := CHECK.match(raw):
            cur = None
            if m.group("glyph") in (" ", "~"):
                cur = {"glyph": m.group("glyph"), "section": section,
                       "scoreboard": scoreboard, "text": [m.group("text").strip()]}
                items.append(cur)
        elif cur is not None and raw.strip() and not raw.lstrip().startswith("- ["):
            cur["text"].append(raw.strip())
    return items


def to_issue(it, seen):
    full = " ".join(it["text"]).strip()
    title = re.sub(r"`", "", re.split(r"\s+—\s+", full, maxsplit=1)[0].strip())
    if len(title) > 80:
        title = title[:77].rstrip() + "..."
    base = f"roadmap:{it['section']}:{slugify(title)}"
    n = seen.get(base, 0)
    seen[base] = n + 1
    ext = base if n == 0 else f"{base}-{n}"
    return {
        "title": title,
        "description": f"{full}\n\nSource: docs/ROADMAP.md ({it['scoreboard']} parity, {it['section']}).",
        "issue_type": "feature",
        "status": "open" if it["glyph"] == " " else "in_progress",
        "priority": 3 if it["glyph"] == " " else 2,
        "labels": ["roadmap", it["scoreboard"], it["section"]],
        "external_ref": ext,
    }


def main() -> int:
    items = parse(ROADMAP.read_text().splitlines())
    seen = {}
    for it in items:
        sys.stdout.write(json.dumps(to_issue(it, seen)) + "\n")
    sys.stderr.write(f"emitted {len(items)} issues from {ROADMAP.name}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
