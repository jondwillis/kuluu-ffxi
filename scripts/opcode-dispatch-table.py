#!/usr/bin/env python3
"""Dump FFXiMain's event-VM opcode dispatch table, cross-referenced with kuluu.

Retail never loads a table of instruction definitions: the mapping from opcode to
the code that performs it is baked into ``FFXiMain.dll`` itself. The interpreter
loop (``f_100bccd0``) reads the exec word ``[event+0x256]``, fetches the opcode
byte and calls ``f_100bc280``, whose body is::

    movsx eax, word ptr [esp + 4]       ; the opcode operand
    cmp   eax, 0da                      ; highest implemented opcode this build
    ja    short L_100bc957              ; out of range -> shared default path
    jmp   dword ptr [eax*4 + 100bc960]  ; THE TABLE: slot index == opcode value

so the table sits at VA ``0x100BC960`` in ``.text``, stride 4, one slot per opcode
``0x00 .. 0xDA`` (219 slots). Each slot holds an eight-byte stub address in the band
``0x100bc297 .. 0x100bc95f``, and each stub body is ``call <handler>; ret 4``: the
handlers are ordinary functions of the same image. DATs carry only the script byte
stream this table interprets, never the definitions themselves.

Slot values are read out of the unpacked image and resolved to their handler by
reading that stub's own ``call`` target out of the generated C, so nothing here is
hand-maintained (the vendor-scrape rule). kuluu coverage comes from the crate
sources: whether an opcode has a constant, and whether any arm uses it.

Usage::

    python scripts/opcode-dispatch-table.py [--out path.md] [--full-stub-column]
"""

from __future__ import annotations

import argparse
import collections
import re
import struct
import sys
from pathlib import Path

# The dispatch of opcode 0x1F is cited in the event records (handler f_100b2ea0),
# so that stub pins the table's first slot: slot[0x1F] == this value.
PINNED_OPCODE = 0x1F
PINNED_STUB = 0x100BC387

# The bound test and the computed jump sit a few instructions apart: `cmp eax, <hi>`
# guards the table jump, so one pattern over that window identifies the dispatcher
# (other `jmp [eax*4 + disp]` sites in the image are unrelated switch tables).
DISPATCH_RE = re.compile(
    r"cmp eax, (0x[0-9a-fA-F]{2})u?.{0,600}?jmp dword ptr \[eax\*4 \+ (0x[0-9A-Fa-f]{8})u?\]",
    re.DOTALL,
)


# (opcode, handler symbol) pairs that other records already pin; they have to land on
# those handlers for a candidate switch table to be the event dispatcher at all.
PIN_PAIRS = [
    (0x1F, "f_100b2ea0"),
    (0x63, "f_100b7550"),
    (0x6E, "f_100b7c40"),
    (0x99, "f_100b7ce0"),
]


def dispatch_site(gen: Path, blob: bytes, stubs: dict[int, tuple[int, str]]) -> tuple[int, int]:
    """(table VA, highest accepted opcode): the switch whose slots resolve to PIN_PAIRS."""
    best: tuple[int, int] | None = None
    for f in sorted(gen.glob("funcs_*.c")):
        text = f.read_text(encoding="utf-8", errors="replace")
        for m in DISPATCH_RE.finditer(text):
            hi, table_va = int(m.group(1), 16), int(m.group(2), 16)
            if best and hi <= best[1]:
                continue
            if validate_pins(blob, stubs):
                best = (table_va, hi)
    if not best:
        raise SystemExit(
            "no `cmp eax, hi` + `jmp [eax*4 + disp]` site resolved every pinned opcode to its "
            "record-cited handler"
        )
    return best


def args_dll() -> str:
    """The image the generated C came from; overridable only through argv order below."""
    if "--dll" in sys.argv:
        return sys.argv[sys.argv.index("--dll") + 1]
    return "research/xi-on-anything/generated/FFXiMain.unpacked.dll"


def validate_pins(blob: bytes, stubs: dict[int, tuple[int, str]]) -> bool:
    """True when every pinned opcode resolves through the slots to its cited handler."""
    try:
        slot0 = slot_zero_offset(blob)
    except SystemExit:
        return False
    for op, want in PIN_PAIRS:
        slot = struct.unpack_from("<I", blob, slot0 + 4 * op)[0]
        got = stubs.get(slot)
        if not got or got[1] != want:
            return False
    return True


def slot_zero_offset(blob: bytes) -> int:
    """File offset of opcode 0's slot, anchored on the pinned stub.

    The dumped image's own headers carry no usable base (it is a memory dump), so
    the anchor is behavioural: the slot whose handler the records already pin sits at
    ``slot0 + 4 * PINNED_OPCODE``, and `verify_slots` confirms the run of slots really
    steps one stub (8 bytes) per opcode. `dispatch_site` only accepts a table VA for
    which that holds, so the two independent sources have to agree.
    """
    hit = blob.find(struct.pack("<I", PINNED_STUB))
    if hit < 0:
        raise SystemExit(f"pinned stub {PINNED_STUB:#x} absent from this image")
    return hit - PINNED_OPCODE * 4


def stub_handlers(gen: Path) -> dict[int, tuple[int, str]]:
    """stub VA -> (handler VA, generated-C symbol). One ``call`` per stub."""
    symbols = {}
    table_c = (gen / "table.c").read_text(encoding="utf-8", errors="replace")
    for va, sym in re.findall(
        r"\{\s*(0x[0-9A-Fa-f]{8})u?,\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}", table_c
    ):
        symbols.setdefault(int(va, 16), sym)

    out: dict[int, tuple[int, str]] = {}
    for f in sorted(gen.glob("funcs_*.c")):
        text = f.read_text(encoding="utf-8", errors="replace")
        if "L_100bc297:" not in text:
            continue
        block = text[text.index("L_100bc297:") :][: 400_000]
        stub = None
        for line in block.splitlines():
            label = re.match(r"L_([0-9A-Fa-f]{8}):", line.strip())
            if label:
                stub = int(label.group(1), 16)
            call = re.search(r"/\* (10[0-9a-f]{6}): call (0x[0-9a-fA-F]{8}) \*/", line)
            if call and stub is not None:
                handler = int(call.group(2), 16)
                out[stub] = (handler, symbols.get(handler, "?"))
                stub = None
    return out


def kuluu_coverage() -> dict[int, tuple[str | None, bool]]:
    """opcode -> (constant name or None, whether an arm uses that constant)."""
    src_dir = Path(__file__).resolve().parent.parent / "ffxi-event" / "src"
    files = [src_dir / "vm.rs", src_dir / "vm" / "scene.rs"]
    blob = "\n".join(f.read_text(encoding="utf-8", errors="replace") for f in files if f.exists())
    consts: dict[int, str] = {}
    for m in re.finditer(r"const OP_([A-Z0-9_]+)(?:: u8)?\s*=\s*(0x[0-9A-Fa-f]{2})", blob):
        consts.setdefault(int(m.group(2), 16), "OP_" + m.group(1))
    out: dict[int, tuple[str | None, bool]] = {}
    for op in range(0x100):
        name = consts.get(op)
        arm = (
            name is not None
            and re.search(rf"\b{name}\b\s*=>|\|\s*{name}\b|{name}\s*\|", blob) is not None
        )
        out[op] = (name, arm)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dll", default="research/xi-on-anything/generated/FFXiMain.unpacked.dll")
    ap.add_argument("--gen", default="research/xi-on-anything/generated/all")
    ap.add_argument("--out", default=None)
    ap.add_argument("--full-stub-column", action="store_true")
    args = ap.parse_args()

    gen = Path(args.gen)
    blob = Path(args.dll).read_bytes()
    band = stub_handlers(gen)
    table_va, hi_opcode = dispatch_site(gen, blob, band)
    slot0 = slot_zero_offset(blob)
    cover = kuluu_coverage()

    shared: dict[int, list[int]] = collections.defaultdict(list)
    buckets: collections.Counter[str] = collections.Counter()
    header_cols = ["op", "handler VA", "handler symbol (this build)", "kuluu constant", "kuluu state"]
    if args.full_stub_column:
        header_cols.insert(1, "stub VA")
    lines = ["| " + " | ".join(header_cols) + " |", "|" + "---|" * len(header_cols)]

    for op in range(hi_opcode + 1):
        slot = struct.unpack_from("<I", blob, slot0 + 4 * op)[0]
        handler = band.get(slot)
        name, arm = cover[op]
        state = "arm" if arm else ("const-only" if name else "unnamed")
        buckets[state] += 1
        cells = [f"{op:02X}"]
        if args.full_stub_column:
            cells.append(f"{slot:#x}")
        cells += [
            (f"{handler[0]:#x}" if handler else "-"),
            (handler[1] if handler else "(shared default path L_100bc957)"),
            name or "-",
            state,
        ]
        lines.append("| " + " | ".join(cells) + " |")
        if handler:
            shared[handler[0]].append(op)

    multi = "; ".join(
        f"{h:#x} <- {', '.join(f'{o:02X}' for o in ops)}"
        for h, ops in sorted(shared.items())
        if len(ops) > 1
    )
    text = (
        # Every line here is a VA or a handler symbol from one build, which the record gate keeps
        # under a `## Provenance` heading; the file has no prose sections to separate it from.
        "## Provenance\n\n"
        f"table VA {table_va:#x}, stride 4, slots 0x00..{hi_opcode:#04X} "
        f"({hi_opcode + 1} opcodes); out-of-range jumps land on the shared default path.\n"
        f"census: {', '.join(f'{k}={v}' for k, v in sorted(buckets.items()))}.\n"
        f"opcodes sharing one handler: {multi or 'none'}\n\n" + "\n".join(lines) + "\n"
    )
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
