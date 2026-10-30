#!/usr/bin/env python3
"""INDEX.md: one line per observation record, with its Kuluu delta tally.

Every record under `.agents/skills/retail-observe/references/` may carry a `## Kuluu delta` section:
the rules that record states, the kuluu symbol responsible for each, and whether it matches, differs or
is missing. This script reads those tables and writes

  | date | record | rules | matches | differs | missing | open beads |

sorted newest first, so a reader can see at a glance which retail laws still have no counterpart in
kuluu. `rules` counts the rows of that table; records predating the convention show "no delta". A bead
id inside a status cell is counted once for its row - ids read as `<prefix>-<number>`, with the prefix
from `.beads/metadata.json`.

Idempotent: same records in, same bytes out. `scripts/checks.sh records-index` regenerates INDEX.md and
fails when it changed, so the index can never go stale. With `--check` nothing is written and the script
itself exits non-zero on a stale index.
"""

import json
import os
import re
import sys

REFS = ".agents/skills/retail-observe/references"
INDEX = os.path.join(REFS, "INDEX.md")
BEADS_METADATA = ".beads/metadata.json"
DEFAULT_BEAD_PREFIX = "kuluu"
STATUSES = ("matches", "differs", "missing")

DATE_IN_NAME = re.compile(r"^(\d{4}-\d{2}-\d{2})-")
DELTA_HEADING = re.compile(r"^## Kuluu delta\s*$")
HEADING = re.compile(r"^#{1,6} ")


def bead_prefix():
    try:
        with open(BEADS_METADATA, encoding="utf-8") as handle:
            database = json.load(handle).get("dolt_database", "")
    except (OSError, ValueError):
        return DEFAULT_BEAD_PREFIX
    return database or DEFAULT_BEAD_PREFIX


def delta_rows(lines):
    """Yield the status cell of each row in a record's Kuluu delta table."""
    inside = False
    for line in lines:
        if DELTA_HEADING.match(line):
            inside = True
            continue
        if not inside:
            continue
        if HEADING.match(line):
            return
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 3:
            continue
        if set(cells[0]) <= set("-: ") or cells[0].lower() == "rule":
            continue
        yield " ".join([cells[2]] + cells[3:])


def tally(lines, prefix):
    """(rules, counts by status, bead ids) for one record; None when it has no delta table."""
    rows = list(delta_rows(lines))
    if not rows:
        return None
    counts = dict.fromkeys(STATUSES, 0)
    beads = set()
    for status in rows:
        lowered = status.lower()
        for name in STATUSES:
            if lowered.startswith(name):
                counts[name] += 1
                break
        beads.update(re.findall(rf"\b{re.escape(prefix)}-\d+\b", status))
    return len(rows), counts, sorted(beads)


def build(prefix):
    entries = []
    for name in sorted(os.listdir(REFS)):
        if not name.endswith(".md") or name == "INDEX.md":
            continue
        match = DATE_IN_NAME.match(name)
        date = match.group(1) if match else ""
        with open(os.path.join(REFS, name), encoding="utf-8") as handle:
            result = tally(handle.read().splitlines(), prefix)
        if result is None:
            entries.append((date, name, "no delta", "", "", "", ""))
            continue
        rules, counts, beads = result
        entries.append(
            (date, name, str(rules), *(str(counts[name]) for name in STATUSES), ", ".join(beads))
        )
    # Two stable sorts: file name as the tie-breaker, then date descending. An undated record has no
    # leading date and so lands last, which is where its kind belongs.
    entries.sort(key=lambda entry: entry[1])
    entries.sort(key=lambda entry: entry[0], reverse=True)

    lines = [
        "# Observation record index",
        "",
        "Written by `scripts/records-index.py`; regenerate with that command, never hand-edit a row.",
        "`rules` counts the rows of a record's Kuluu delta table and `open beads` lists the gap beads it",
        "cites. Records from before the delta convention show \"no delta\".",
        "",
        "| date | record | rules | matches | differs | missing | open beads |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    lines += ["| " + " | ".join(entry) + " |" for entry in entries]
    return "\n".join(lines) + "\n", len(entries)


def main(argv):
    flags = argv[1:]
    body, count = build(bead_prefix())
    current = None
    if os.path.exists(INDEX):
        with open(INDEX, encoding="utf-8") as handle:
            current = handle.read()
    if current == body:
        print(f"records-index: {INDEX} up to date ({count} records)")
        return 0
    if "--check" in flags:
        print(f"records-index: {INDEX} is stale ({count} records); run python3 scripts/records-index.py", file=sys.stderr)
        return 1
    with open(INDEX, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(body)
    print(f"records-index: wrote {INDEX} ({count} records)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
