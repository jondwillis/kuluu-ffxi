#!/usr/bin/env python3
"""Observation records keep binary-level detail under a `## Provenance` heading.

A retail-observe record states what retail does in interop terms: DAT fields,
wire fields, on-screen effect, timing. Addresses, decompiler function names,
instruction streams, vtable slots and in-memory field offsets are the evidence
trail for that statement, not the statement itself, so they may appear only in
the section headed `## Provenance`. The implementation is then written from the
spec sections alone, which is what keeps it an independent implementation
rather than a transcription of the disassembly.

Usage: record-provenance.py [--self-test] FILE...
Prints `file:line: text` for every offending line and exits 1 if any.
"""

import re
import sys

PROVENANCE_HEADING = "provenance"

REGISTER = r"(?:e?[abcd]x|[abcd][lh]|e?[sd]i|e?[sb]p)"
BINARY_DETAIL = re.compile(
    r"\b(?:RVA|VA)\s+`?0x[0-9A-Fa-f]+"
    r"|\b0x10[0-9A-Fa-f]{6}\b"
    r"|\bFUN_[0-9A-Fa-f]{6,}\b"
    r"|\+\s?`?0x[0-9A-Fa-f]{2,}\b"
    r"|\bvtable slot\b"
    r"|\b(?:mov|movzx|lea|cmp|test|shr|shl|sar|and|or|xor|add|sub|jmp|call)\s+"
    + REGISTER
    + r"\b\s*,"
)
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")

SELF_TEST_BAD = [
    "The normalizer at RVA `0x8EF70` reads the live sub.",
    "the trailing token from an un-decompiled routine 0x100198f0",
    "app.dll FUN_1019ab29 derives the member-login secret",
    "compares RF1 bits 1-3 against actor+0x8A9",
    "plays `table[sub]` via vtable slots +0x29C and/or +0x298",
    "the dispatch sites shift right one and mask with seven (`shr eax,1; and al,7`)",
    "called via the setSub vtable slot",
]
SELF_TEST_GOOD = [
    "and so, the spawn flag is absorbed by the table index",
    "test it, then compare the capture",
    "the 19-bit size field sits at offset 0x1C of the chunk header",
    "zone-230 DialogTable entries 6428/6434/6437",
    "hands the game a 16-byte value and a 64-byte authCode",
    "arrives inside the s2c 0x00A LOGIN body",
    "race 1 action base 38603, fishing DAT 38604 (ROM/25/39)",
    "the mask 0x7FFFF selects the size units",
]


def offending_lines(text):
    section = None
    for number, line in enumerate(text.splitlines(), start=1):
        heading = HEADING.match(line)
        if heading and len(heading.group(1)) <= 2:
            section = heading.group(2).strip().lower()
            continue
        if section == PROVENANCE_HEADING:
            continue
        if BINARY_DETAIL.search(line):
            yield number, line


def self_test():
    for line in SELF_TEST_BAD:
        if not BINARY_DETAIL.search(line):
            print(f"record-provenance self-test: did not fire on: {line}", file=sys.stderr)
            return False
    for line in SELF_TEST_GOOD:
        if BINARY_DETAIL.search(line):
            print(f"record-provenance self-test: fired on spec prose: {line}", file=sys.stderr)
            return False
    spec_then_provenance = "# Title\n\n## Behavior\n\nfoo\n\n## Provenance\n\nRVA 0x1234\n"
    if list(offending_lines(spec_then_provenance)):
        print("record-provenance self-test: flagged an address under Provenance", file=sys.stderr)
        return False
    leaked = "# Title\n\n## Provenance\n\nRVA 0x1234\n\n## Behavior\n\nRVA 0x5678\n"
    if [n for n, _ in offending_lines(leaked)] != [9]:
        print("record-provenance self-test: missed an address after Provenance ended", file=sys.stderr)
        return False
    return True


def main(argv):
    if argv and argv[0] == "--self-test":
        return 0 if self_test() else 1
    bad = 0
    for path in argv:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        for number, line in offending_lines(text):
            print(f"{path}:{number}: {line.strip()[:160]}")
            bad += 1
    if bad:
        print(
            f"records: {bad} line(s) carry binary-level detail outside a `## Provenance` section;"
            " move addresses, offsets, decompiler names and instruction streams there and keep"
            " the spec sections in interop terms (DAT fields, wire fields, on-screen effect).",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
