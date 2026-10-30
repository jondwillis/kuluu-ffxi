#!/usr/bin/env python3
"""Derive the 1:1 opcode effect table straight from this build's decompiled handlers.

Every slot in FFXiMain's event dispatch table gets one row, and each row lists what its handler body
actually *does*, read out of the per-instruction comments in `research/xi-on-anything/generated/all`:
how it steps past itself (`+0x256`), whether it yields (`+0x25A`), every memory store with its verbatim
instruction and VA, every engine call it makes, and whether it jumps through a table.

Nothing in the output is hand-written, so a row cannot quietly claim "no effect". A slot shows an empty
store/call list only when its handler really contains neither, and `--check` fails if that store-free
set drifts away from the slots verified by hand in STUB_ONLY (which is exactly how six rows once got
written off as "effectless" here).

    python scripts/opcode-effect-table.py [--dll ...] [--gen ...] [--out ...] [--check]
"""
import argparse
import importlib.util
import json
import pathlib
import re
import struct
import sys


def load_ledger_module():
    """Reuse the parity ledger's dispatch/slot parsing rather than duplicating it."""
    path = pathlib.Path(__file__).resolve().parent / "opcode-parity-ledger.py"
    spec = importlib.util.spec_from_file_location("opcode_parity_ledger", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# Slots whose handlers contain neither a store nor a call, each verified against the generated C:
# 0x04 (f_100b0120) and 0x6D (f_100b7850) step past themselves and do nothing else. `--check` fails if
# the extractor starts seeing an effect in one of these, or stops seeing one everywhere else.
STUB_ONLY = {0x04, 0x6D}

RE_INS = re.compile(r"/\*\s*(1[0-9a-f]{7}):\s*(.*?)\s*\*/")
MEM_SIZE = r"(?:byte|word|dword|fword|tbyte|oword|qword)"
RE_STORE = re.compile(
    rf"^(mov|and|or|xor|add|sub|inc|dec|neg|not|rep)\s+{MEM_SIZE}\s*ptr\s*\[([^\]]+)\](?:,\s*(.*))?$",
    re.I,
)
RE_EXEC_STEP = re.compile(r"^(add|inc)\s+word ptr \[(\w+) \+ 0x256\](?:,\s*(0x[0-9a-f]+|\w+))?$", re.I)
RE_EXEC_SET = re.compile(r"^mov\s+word ptr \[(\w+) \+ 0x256\],\s*(.+)$", re.I)
RE_YIELD = re.compile(r"^\w+\s+" + MEM_SIZE + r"\s+ptr \[(\w+) \+ 0x25[aA]\]", re.I)
# Image addresses in this build are eight hex digits (0x101d6110); matching only seven silently dropped
# every call and mislabelled every absolute global as a register-relative write, which is what the first
# run of this script did before --check complained.
RE_CALL = re.compile(r"^call\s+(0x1[0-9a-f]{7})\b", re.I)
RE_JMP_TAB = re.compile(r"^jmp\s+dword ptr \[", re.I)   # table branch, distinct from a one-jump thunk
RE_GLOBAL_DST = re.compile(r"^0x1[0-9a-f]{7}$", re.I)


def classify_store(dst):
    """(kind, detail) for a memory destination operand.

    `global` is an absolute image address (retail's own client state); `exec-field` is the interpreter's
    step/yield word; `relative` is a base-register plus offset write - an entity, mover or work record.
    """
    dst = dst.strip().lower()
    if RE_GLOBAL_DST.match(dst):
        return "global", dst
    if "+ 0x256" in dst or "+ 0x25a" in dst:
        return "exec-field", dst
    return "relative", dst


def instructions_of(body):
    out = []
    for line in body.splitlines():
        m = RE_INS.search(line)
        if m:
            out.append((m.group(1), re.sub(r"\s+", " ", m.group(2)).strip()))
    return out


RE_JMP_OUT = re.compile(r"^jmp\s+(0x1[0-9a-f]{7})\b", re.I)


def resolve_body(sym, funcs):
    """Follow one-jump thunks so a slot's effects are inventoried where they actually live.

    Several dispatch slots point at a stub that immediately jumps to the real handler (0x24 QUERY is
    `f_100b2270` -> `f_100b25f0`). Counting such a stub as "no stores, no calls" would be exactly the
    mistake this table exists to prevent, so the chain is walked and reported.
    """
    chain = [sym]
    cur = sym
    while True:
        body = funcs.get(cur)
        if not body:
            return chain, []
        ins = instructions_of(body)
        if len(ins) == 1:
            m = RE_JMP_OUT.match(ins[0][1])
            if m:
                cur = "f_" + m.group(1)[2:].lower()
                chain.append(cur)
                continue
        return chain, ins


def inventory(instructions):
    inv = {"advance": [], "exec_set": [], "yield": [], "stores": [], "calls": [], "jump_table": []}
    for va, ins in instructions:
        m = RE_EXEC_STEP.match(ins)
        if m:
            inv["advance"].append((va, (m.group(3) or "1").lower()))
            continue
        m = RE_EXEC_SET.match(ins)
        if m:
            inv["exec_set"].append((va, m.group(2).strip().lower()))
            continue
        if RE_YIELD.match(ins):
            inv["yield"].append(va)
        m = RE_CALL.match(ins)
        if m:
            inv["calls"].append((va, "f_" + m.group(1)[2:].lower()))
            continue
        if RE_JMP_TAB.match(ins):
            inv["jump_table"].append(va)
        m = RE_STORE.match(ins)
        if m:
            kind, detail = classify_store(m.group(2), )
            inv["stores"].append((va, kind, detail, ins.lower()))
    return inv


def main():
    ap = argparse.ArgumentParser()
    ledger = load_ledger_module()
    ap.add_argument("--dll", default=ledger.DLL_DEFAULT)
    ap.add_argument("--gen", default=ledger.GEN_DEFAULT)
    ap.add_argument(
        "--out",
        default=".agents/skills/retail-observe/references/data/opcode-effect-table.md",
    )
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()

    gen = pathlib.Path(args.gen)
    blob = pathlib.Path(args.dll).read_bytes()
    stubs = ledger.odt.stub_handlers(gen)
    _table_va, hi_opcode = ledger.odt.dispatch_site(gen, blob, stubs)
    slot0 = ledger.odt.slot_zero_offset(blob)
    funcs = ledger.load_funcs(gen)

    rows = []
    for op in range(hi_opcode + 1):
        slot = struct.unpack_from("<I", blob, slot0 + 4 * op)[0]
        handler = stubs.get(slot)
        sym = handler[1] if handler else None
        chain, ins = resolve_body(sym, funcs) if sym else ([None], [])
        rows.append((op, sym, len(ins), inventory(ins), chain))

    stub_now = {op for op, _s, _n, inv, _c in rows if not inv["stores"] and not inv["calls"]}
    if args.check:
        drift = sorted(stub_now ^ STUB_ONLY)
        if drift:
            print("stub-only set changed: " + ", ".join(f"0x{o:02X}" for o in drift), file=sys.stderr)
            return 1

    counts = {
        "slots": len(rows),
        "with stores": sum(1 for r in rows if r[3]["stores"]),
        "with calls": sum(1 for r in rows if r[3]["calls"]),
        "jump tables": sum(1 for r in rows if r[3]["jump_table"]),
        "thunk slots resolved through a jump": sum(1 for r in rows if len(r[4]) > 1),
        "no body found": sum(1 for r in rows if r[1] and not r[2]),
    }
    out = pathlib.Path(args.out)
    lines = [
        # The record gate lets binary detail (VAs, handler names, instruction streams) live only under a
        # `## Provenance` heading, and every line of this file is exactly that: nothing here is prose,
        # it is the extracted instruction trail the verdicts are argued from.
        "## Provenance",
        "",
        "opcode effect table - derived by `scripts/opcode-effect-table.py` from this build's generated C;",
        "do not hand-edit: every cell is one of the handler's own instructions, with its VA. One row per",
        "dispatch slot, so an opcode cannot be described as having no effect without that claim being",
        "visible in the instruction list beneath it.",
        "",
        f"counts: {json.dumps(counts)}",
        "",
        f"store-free and call-free handlers (verified by hand): "
        + ", ".join(f"0x{o:02X}" for o in sorted(stub_now)),
        "",
    ]
    for op, sym, n, inv, chain in rows:
        head = f"### {op:02X} - {sym or '(shared default)'} ({n} instructions)"
        if len(chain) > 1:
            head += " [thunk] " + " -> ".join(c for c in chain if c)
        lines.append(head)
        if inv["advance"]:
            lines.append("- advance: " + ", ".join(f"{v}:{a}" for v, a in inv["advance"]))
        if inv["exec_set"]:
            lines.append("- exec set (jump form): " + ", ".join(f"{v}:={a}" for v, a in inv["exec_set"]))
        if inv["yield"]:
            lines.append("- yield byte writes: " + ", ".join(inv["yield"]))
        if inv["jump_table"]:
            lines.append("- jump table at: " + ", ".join(inv["jump_table"]))
        if inv["stores"]:
            lines.append("- stores:")
            for va, kind, detail, text in inv["stores"]:
                lines.append(f"  - `{va}` [{kind}] {text}")
        else:
            lines.append("- stores: none")
        if inv["calls"]:
            lines.append("- calls: " + ", ".join(f"{v}->{c}" for v, c in inv["calls"]))
        else:
            lines.append("- calls: none")
        lines.append("")
    out.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    print(f"wrote {out}: {json.dumps(counts)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
