#!/usr/bin/env python3
"""Print an emulated function from research/xi-on-anything/generated/all as its asm-comment list.

Each handler body in the generated C carries one ``/* VA: instruction */`` comment per machine
instruction; this dumps exactly those, so a handler's exec-step / yield-byte / work-slot behavior
can be read straight off without wading through the emulation scaffolding. Usage::

    python scripts/opcode-body.py f_100b04c0 [f_...] [--funcs-dir PATH]
"""

from __future__ import annotations

import argparse
import pathlib
import re


def load_funcs(gen: pathlib.Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for f in sorted(gen.glob("funcs_*.c")):
        text = f.read_text(encoding="utf-8", errors="replace")
        for m in re.finditer(r"^void (f_10[0-9a-fA-F]{6})\(Guest\* g\)\n\{", text, re.M):
            start = m.end() - 1
            end = text.find("\n}\n", start)
            out.setdefault(m.group(1), text[start : (end if end >= 0 else len(text)) + 3])
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("syms", nargs="+")
    ap.add_argument("--funcs-dir", default="research/xi-on-anything/generated/all")
    args = ap.parse_args()
    funcs = load_funcs(pathlib.Path(args.funcs_dir))
    for sym in args.syms:
        body = funcs.get(sym)
        print(f"===== {sym} =====")
        if body is None:
            print("NOT FOUND in generated C")
            continue
        for va, ins in re.findall(r"/\* (10[0-9a-f]{6}): (.+) \*/", body):
            print(f"{va}: {ins}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
