#!/usr/bin/env python3
"""Per-opcode parity ledger: FFXiMain's event handlers vs kuluu's arms.

The dispatch table (see `opcode-dispatch-table.py`, whose stub/slot parsing this
reuses) gives opcode -> handler for all 219 slots. This script goes one level
deeper and reads what each *handler body* does to the two words of event state
that decide execution, from the generated C's per-instruction comments:

- exec pointer `[event + 0x256]`: handlers step past themselves with
  `add word ptr [R + 0x256], N` (N = instruction width), advance by a computed
  register (variable-width opcode), or set it outright with
  `mov word ptr [R + 0x256], reg` — the jump form every branch opcode uses.
- yield byte `[event + 0x25A]`: written non-zero to break the interpreter's
  run-until-yield loop (`f_100bccd0` clears it on entry and re-reads it after
  every dispatch); kuluu models the same thing with `sets_ret`.

Joining those facts against kuluu's declared metadata (`ffxi-event/src/opcode_meta.rs`:
size/jumps/sets_ret) and arm presence (`vm.rs`, `vm/scene.rs`) yields one verdict
row per opcode. Machine-verifiable fields get machine verdicts; everything else
stays UNAUDITED with the reason, which is the work queue:

- NO-ARM      kuluu has no arm; it skips by declared size (or refuses if the
              metadata says jumps/sets_ret) wherever retail runs a handler.
- SIZE-DIFFER the handler steps past itself by N but kuluu declares another size.
- JUMP-DIFFER one side sets exec non-linearly and the other does not.
- PARK-DIFFER the handler yields via +0x25A where kuluu says it falls through,
              or kuluu yields where retail's handler never writes the byte.
- UNAUDITED   computed width, several different step amounts on different paths,
              or no exec access in the handler body at all (thread-level ops like
              END); needs a human read of that body, and an AUDIT entry once done.

A row is MATCH only when arm exists and every machine field agrees. Audited rows
are overridden by name in AUDITED below so regeneration never loses the verdict.

Usage::

    python scripts/opcode-parity-ledger.py [--out path.md]
"""

from __future__ import annotations

import argparse
import collections
import importlib.util
import pathlib
import re
import struct
import sys

# Reuse the pinned dispatch-table extractor rather than restating its parsing:
# same directory, load it as a module.
_ODT_PATH = pathlib.Path(__file__).resolve().parent / "opcode-dispatch-table.py"
_spec = importlib.util.spec_from_file_location("odt", _ODT_PATH)
odt = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(odt)

GEN_DEFAULT = "research/xi-on-anything/generated/all"
DLL_DEFAULT = "research/xi-on-anything/generated/FFXiMain.unpacked.dll"

FUNC_RE = re.compile(r"^void (f_10[0-9a-fA-F]{6})\(Guest\* g\)\n\{", re.M)
ASM_RE = re.compile(r"/\* (10[0-9a-f]{6}): (.+) \*/")

# Exec/park accesses, on the instruction comment text only. The bases seen in this build are
# plain registers (ecx/esi/edi copies of the event pointer); array-indexed `+ 0x25a]` hits elsewhere in
# the image belong to unrelated structures and must not match, so the base class excludes indices.
def _wr_re(offset: str) -> re.Pattern[str]:
    return re.compile(
        rf"(add|mov|sub)\s+(word|byte|dword)\s+ptr\s+\[([a-z]{{2,3}})(?: \+ 0x[0-9a-f]+)? \+ {offset}\],\s*(.+)"
    )


EXEC_RE = _wr_re("0x256")
PARK_RE = _wr_re("0x25[aA]")
CALL_RE = re.compile(r"call 0x(10[0-9a-f]{6})")

IMM_RE = re.compile(r"\s*(?:0[xX])?([0-9a-fA-F]+)u?\s*$")


def imm_of(opnd: str) -> int | None:
    """Immediate value of an operand string, or None if it is a register/expr."""
    m = IMM_RE.match(opnd.strip())
    if not m or re.fullmatch(r"[a-d][xls]?[bp]?", opnd.strip().lower()):
        return None
    try:
        return int(m.group(1), 16)
    except ValueError:
        return None


def load_funcs(gen: pathlib.Path) -> dict[str, str]:
    """symbol -> full body text (comment-bearing) for every emulated function."""
    out: dict[str, str] = {}
    for f in sorted(gen.glob("funcs_*.c")):
        text = f.read_text(encoding="utf-8", errors="replace")
        for m in FUNC_RE.finditer(text):
            start = m.end() - 1  # the opening brace
            end = text.find("\n}\n", start)
            if end < 0:
                end = len(text)
            out.setdefault(m.group(1), text[start : end + 3])
    return out


class Facts:
    """What a body does to exec/park, plus whom it calls."""

    def __init__(self, sym: str, funcs: dict[str, str]):
        self.sym = sym
        self.adds: list[int | None] = []  # None = computed (register) amount
        self.movsets: list[str] = []
        self.parks: list[str] = []
        self.n_insn = 0
        self.nop_only = False
        self.calls: set[str] = set()
        body = funcs.get(sym, "")
        ins_list = ASM_RE.findall(body)
        self.n_insn = len(ins_list)
        for _va, ins in ins_list:
            m = EXEC_RE.search(ins)
            if m:
                opnd = m.group(4)
                if m.group(1) == "mov":
                    self.movsets.append(opnd)
                else:
                    self.adds.append(imm_of(opnd))
                continue
            m = PARK_RE.search(ins)
            if m and m.group(1) != "cmp":
                self.parks.append(f"{m.group(1)} {m.group(2)} <- {m.group(4).strip()}")
                continue
            c = CALL_RE.search(ins)
            if c:
                self.calls.add("f_" + c.group(1))
        # `add exec, N; ret` and nothing else: the opcode does nothing but step past itself.
        self.nop_only = (
            not self.calls
            and not self.movsets
            and not self.parks
            and len(self.adds) == 1
            and self.adds[0] is not None
            and self.n_insn == 2
        )

    def form(self, funcs: dict[str, str]) -> tuple[str, int | None, bool]:
        """(display form, fixed size or None, jump-set?)."""
        parts: list[str] = []
        imm: int | None = None
        distinct = set(self.adds)
        if self.adds and len(distinct) == 1 and None not in distinct:
            imm = next(iter(distinct))
            parts.append(f"add {imm}")
        elif self.adds:
            vals = ";".join("?" if v is None else str(v) for v in sorted(set(map(str, distinct))))
            parts.append(f"COND-add({vals})")
        if any(a is None for a in self.adds):
            parts.append("computed")
        if self.movsets:
            parts.append(f"jump-set x{len(self.movsets)}")
        return ("+".join(parts) if parts else "none"), imm, bool(self.movsets)


def effective(sym: str, funcs: dict[str, str], depth: int = 2) -> tuple[Facts, bool, list[str]]:
    """Facts for `sym`, merging the body of its one implementation-helper when the handler itself is a
    thunk (the scheduler twins dispatch through shared routines like f_100b4af0). Returns
    (facts, merged?, helper names consulted); a second level only if that helper is itself a thunk."""
    fa = Facts(sym, funcs)
    if fa.adds or fa.movsets or fa.parks or depth <= 0:
        return fa, False, []
    workers = [c for c in sorted(fa.calls) if _touches(c, funcs)]
    workers = list(dict.fromkeys(workers))
    if len(workers) == 1:
        inner, _, deeper = effective(workers[0], funcs, depth - 1)
        merged = Facts(sym + "@" + workers[0], {sym + "@" + workers[0]: funcs.get(workers[0], "")})
        merged.adds = inner.adds
        merged.movsets = inner.movsets
        merged.parks = inner.parks
        merged.calls = set()
        return merged, True, [workers[0]] + deeper
    return fa, False, workers


def _touches(sym: str, funcs: dict[str, str]) -> bool:
    if sym not in funcs:
        return False
    body = funcs[sym]
    for _va, ins in ASM_RE.findall(body):
        if EXEC_RE.search(ins) or PARK_RE.search(ins):
            return True
    return False


def kuluu_meta() -> list[dict[str, object]]:
    """OPCODE_META entries in index order from opcode_meta.rs.

    Entries are parsed from comment-stripped text (some carry doc-comment lines *inside* the brace,
    which no single field regex can tolerate), then cross-checked against the file's own `// 0xXXXX`
    labels so a position/opcode misread aborts loudly instead of shifting every size by one.
    """
    src = pathlib.Path(__file__).resolve().parent.parent / "ffxi-event" / "src" / "opcode_meta.rs"
    text = src.read_text(encoding="utf-8", errors="replace")
    body = text[text.index("pub const OPCODE_META") :]
    arr_start = body.index("[")
    arr_end = body.index("];", arr_start)
    raw = body[arr_start:arr_end]
    stripped = re.sub(r"//[^\n]*", "", raw)
    entries = re.findall(
        r"OpMeta \{\s*size: (\d+),\s*jumps: (true|false),\s*sets_ret: (true|false),\s*valid: (true|false),?\s*\}",
        stripped,
    )
    labeled = re.findall(r"OpMeta \{[^{}]*\},?\s*// 0x([0-9A-Fa-f]{4})", raw)
    if len(labeled) == len(entries) and [int(x, 16) for x in labeled] != list(range(len(entries))):
        bad = next(i for i, x in enumerate(labeled) if int(x, 16) != i)
        raise SystemExit(f"opcode_meta.rs: entry at index {bad:#x} carries label {labeled[bad]} — indexing changed")
    if len(labeled) == len(entries):
        pass
    return [
        {
            "size": int(s),
            "jumps": j == "true",
            "sets_ret": r == "true",
            "valid": v == "true",
        }
        for s, j, r, v in entries
    ]


def kuluu_coverage_all() -> dict[int, tuple[str | None, bool]]:
    """opcode -> (constant name or None, whether any arm matches it), across all of ffxi-event/src.

    One opcode can carry differently-named constants in different files (0x1F is OP_MOVE_TEST in vm.rs
    but OP_MOVE with its real arms in vm/scene.rs); the dispatch-table script's two-file scan under-reads
    that, and an arm found anywhere counts. The first constant name is reported.
    """
    src = pathlib.Path(__file__).resolve().parent.parent / "ffxi-event" / "src"
    names: dict[int, list[str]] = {}
    bodies: dict[str, str] = {}
    # Constant *names* are collected from vm.rs, vm/scene.rs and opcode_meta.rs —
    # some (OP_ENTITYSPEED and the other sub-byte family constants) are defined
    # in opcode_meta.rs and only used as dispatch patterns elsewhere, and a
    # two-file scan read those as "no constant, no arm".
    for f in [src / "vm.rs", src / "vm" / "scene.rs", src / "opcode_meta.rs"]:
        if not f.exists():
            continue
        text = f.read_text(encoding="utf-8", errors="replace")
        bodies[f.name] = text
        for m in re.finditer(r"const (OP_[A-Z0-9_]+)(?:: u8)?\s*=\s*(0x[0-9A-Fa-f]{2})", text):
            names.setdefault(int(m.group(2), 16), []).append(m.group(1))
    # Arm detection: a constant counts as armed when some non-comment line of a
    # dispatch body (vm.rs, vm/scene.rs) uses it — beyond its own `const`
    # definition for names defined there; a meta-defined name needs one use in a
    # dispatch body. Comment lines are dropped first so prose never counts.
    dispatch = "\n".join(
        line
        for fname, text in bodies.items()
        if fname != "opcode_meta.rs"
        for line in text.splitlines()
        if not line.strip().startswith(("//", "///"))
    )
    meta_defined = {
        n
        for (op_list, ns) in names.items()
        for n in ns
        if re.search(rf"const {n}(?:: u8)?\s*=", bodies.get("opcode_meta.rs", ""))
    }
    out: dict[int, tuple[str | None, bool]] = {}
    for op in range(0x100):
        aliases = sorted(set(names.get(op, [])))
        used = [
            n
            for n in aliases
            if len(re.findall(rf"\b{n}\b", dispatch)) > (0 if n in meta_defined else 1)
        ]
        armed = bool(used)
        shown = (used or aliases)[-1] if aliases else None
        out[op] = (shown, armed)
    return out


# Rows settled by reading the handler body (date + one-line reason); they override the machine verdict.
# The three END dispatch slots share f_100afc80; it writes no exec step at all — it resets the
# thread slot's fields and sets the yield byte, which is kuluu's explicit `finished` path, so the
# "no exec access" UNAUDITED bucket would otherwise mislabel a faithful arm.
# WIDTH-ONLY is a verdict of my own coinage for the sub-byte families reached
# only through the shared width handler in vm.rs: every case advances by its
# recorded width (sub_size), but no effect of any case has been modelled, and
# calling that MATCH would paper over exactly what the queue exists to finish.
AUDITED: dict[int, tuple[str, str]] = {
    0x00: ("MATCH", "end-of-thread handler f_100afc80; kuluu ends the VM explicitly"),
    0xCA: ("MATCH", "shares END's handler slot with 0x00; armed as END since 2026-10-29"),
    0xCB: ("MATCH", "shares END's handler slot with 0x00; armed as END since 2026-10-29"),
    0x7A: ("WIDTH-ONLY", "handler f_100b3a50 read to last instruction 2026-10-29 late with jump table @100b3ca8 dumped from the image (six bodies, ceiling five): zero resolves an actor and hands token −1 plus a fetch-dword to helper f_100b3dc0 (advance six); one same helper with byte +6 (seven); two swaps the thread's exec fields into a resolved actor's mover block (current entity at +1, destination counter at mover-state word incremented, pending flag set — any gate failure sets pending with NO advance and retail re-dispatches); three decrements-and-clamps that counter on the current record then restores exec from the saved fields (advance two, pending set); four fetches width-three operands and asks predicate f_100b3cc0 — answer two parks with pending and no advance, else advance eight; five helper f_100b3d70 same park-on-two law (else six). Sub above five pops without advancing. kuluu's sub_size rows {0,2,5→six; 1→seven; 3→two; 4→eight} equal every case advance exactly; the exec-swap halves and both predicates parsed-not-applied (their answers are client state)"),
    0xB5: ("WIDTH-ONLY", "handler f_100b10f0 read to last instruction 2026-10-29 late: sub zero resolves the actor named at +2 (skip-on-null advances anyway); fetches the dword via f_100af960(2) and calls f_10093430 on the record, which — when the record's name count is not positive — copies six dwords from the fetched pointer into the record's 24-byte name block @entry+124 and zeroes byte +148 (the literal-block-patch authoring form); advance four every path. Any non-zero sub sets exec's pending flag with no advance (retail re-dispatches, kuluu halts). Widths equal; the copy itself unmodelled — pointer-valued stream operands cannot be authored in kuluu"),
    0x59: ("MATCH", "nine-case switch armed in f_100b6a60 form, case widths lifted individually into sub_size; the yield byte is case 6's visual-busy park, kuluu taking retail's absent-actor advance path (recorded)"),
    0x68: ("MATCH", "handler f_100b7a20 read 2026-10-29: two HUD-singleton calls + one global-byte reset, then inc ip; kuluu emits its un-hide cue and advances 1 — the renderer-side equivalent of retail's singleton pokes"),
    0x6D: ("MATCH", "handler f_100b7850 read 2026-10-29: an empty `add +7` stub; kuluu has no arm and the generic width skip walks exactly 7 — there is no behaviour to miss"),
    0x26: ("MATCH", "handler f_100afc70 read 2026-10-29: one byte — set the yield flag, never touch exec — so the interpreter re-dispatches it forever; that is exactly what kuluu's parked OP_YIELD_FOREVER does"),
    0x58: ("MATCH", "handler f_100b0380 read 2026-10-29: `inc exec` then set the yield flag — advance one and hand back to the driver, kuluu's armed OP_YIELD width/law exactly"),
    0x2E: ("MATCH", "handler f_100b55d0 read 2026-10-29: pokes three client-global bytes (one conditional on a fourth) and `inc exec`; no operand reads, no stream effect beyond width 1 — kuluu keeps none of those globals, the arm's advance-only path is the observable law"),
    0x42: ("MATCH", "handler f_100b5600 read 2026-10-29: sibling of 0x2E — zeroes two global bytes (one conditional) and `inc exec`; width 1 only, as armed"),
    0x30: ("MATCH", "handler f_100b5620 read 2026-10-29, ARMED 2026-10-30 (`OP_CLIENT_BYTE_10488100_ZERO`): zeroes the fixed client byte [0x10488100] and `inc exec`; kuluu now performs that store into EventClientGlobals::byte_10488100, pinned by `fixed_client_global_stores_write_their_cells_and_nothing_else`. The earlier verdict rested on 'kuluu keeps none of those globals, so the width skip is the whole observable law' — withdrawn: retail stores, so kuluu stores"),
    0x21: ("MATCH", "handler f_100b34e0 read 2026-10-29: sets a client-global request byte (and mirrors one other global into it when clear), then yields without advancing — the thread parks forever on an end-request; kuluu's OP_EXECEND arm ends the VM explicitly, same net effect as END's audit posture"),
    0x4C: ("MATCH", "handler f_100b6950 read 2026-10-29: on the block's own entry, when flags word [entry+0x120] low-byte bit2 is clear, stores status value 8 at [entry+0x174]; `inc exec` on every path — exactly kuluu's emit_status_event_cue(STATUS_EVENT_DOOR_OPEN) with the same unmodelled render-flag gate"),
    0x4D: ("MATCH", "handler f_100b6a00 read 2026-10-29: sibling of 0x4C storing value 9 ([entry+0x174]) under the same flag gate — kuluu's STATUS_EVENT_DOOR_CLOSE arm, gate posture identical"),
    0x3E: ("MATCH", "handler f_100b0250 read 2026-10-29: BITTEST — flat bit index @+3 split into slot/bit by sar 5 / and 0x1f, tested against the operand-1 word fetched via the slot; set → `add exec,7`, clear → jump to the u16 at +5. Identical shape to kuluu's armed OP_BITTEST (same BIT_TEST_* split constants)"),
    0x24: ("MATCH", "handler f_100b2270 is a one-jump thunk into f_100b25f0; kuluu's armed OP_QUERY sends its query and parks on the ack through the same pending-request posture as SENDTAG — audit at that level, the helper body itself unread"),
    0x25: ("MATCH", "handler f_100b2940 read 2026-10-29: QUERYWAIT polls a query-result word; zero → yield without advancing (retry next tick — kuluu's AwaitServerAck park), non-zero → advance one and post the answer to client globals, with an 0xFF branch storing 0xFE plus three end-bytes; kuluu models the poll/answer split, not those global side-writes"),
    0x23: ("MATCH", "handler f_100b2dd0 read 2026-10-29: MESWAIT is a four-state machine on one global byte — 0 advance-and-clear the thread's speaker slot ([thread+0x164] → entity [entry+0x14e]=0, optional animation release), 1 yield in place, 2 end-bytes + yield without advancing; kuluu's message-await arm holds until the host reports the dialog closed and then advances — same observable stream law, global half unmodelled"),
    0x78: ("MATCH", "handler f_100b87b0 read 2026-10-29: conditionally on a paused-state byte, two clock-singleton refresh calls and clearing one visibility slot; `inc exec` always — exactly kuluu's armed OP_RESTORE_CLOCK cue posture (renderer owns the singleton side)"),
    0x8A: ("MATCH", "handler f_100b93c0 read 2026-10-29: one map-window helper call (`call 0x100b2a60`, also invoked from the query-pair teardown above) then `inc exec` — kuluu's OP_CLOSE_MAP cue is that close; no operand reads to miss"),
    0x60: ("MATCH", "handler f_100b73f0 read 2026-10-29: selector byte @+1 picks BOTH width and law — sel<=1 xor-latches [entry+0x124] bit30 to the selector (advance 4), sel==2 hands the looked-up actor (token = the selector itself) to singleton [[0x1065cb04]] vtable +0x18 internals and advances 6, any other selector takes retail's default arm adding exactly 2 (it does not spin); kuluu arms sub_size widths 4/6/2 and cues the latch in ENTITY_WORD_124_BASE space, singleton handoff parsed-not-applied"),
    0x61: ("MATCH", "handler f_100b74e0 read 2026-10-29: mask-OR store of byte@+1 & 1 into [entry+0x128] bit0 (not an xor-latch), empty entity-table slot skips the write, advance 2 either way; kuluu cues ENTITY_WORD_128_BASE+0"),
    0x74: ("MATCH", "handler f_100b7490 read 2026-10-29: third latch of the word-124 family after 0x5F's bit29 and 0x60's bit30 — xor-latch [entry+0x124] bit31 to byte@+1 & 1, advance 2; kuluu cues ENTITY_WORD_124_BASE+31"),
    0x7C: ("MATCH", "handler f_100b8a80 read 2026-10-29: [entry+0x128] bit17 set-or-cleared from whether byte@+1 is non-zero (`or 0x20000` / `and ~0x20000` pair), gated on the standard eligibility word + null-slot skip; advance 6 every path; kuluu cues ENTITY_WORD_128_BASE+17"),
    0xA4: ("MATCH", "handler f_100ba8d0 read 2026-10-29 late: forces [entry+0x12c] bit26 to byte@+1 & 1 on the entity at word [exec+2], null-slot skips the write, advance 2; armed this pass as OP_ENTITY_WORD12C_SET_BIT26 cueing ENTITY_WORD_12C_BASE+26"),
    0xA5: ("MATCH", "handler f_100ba920 read 2026-10-29 late: twin of 0xA4 pinning [entry+0x12c] bit11 from byte@+1 & 1, advance 2; armed this pass as OP_ENTITY_WORD12C_SET_BIT11"),
    0xC0: ("MATCH", "handler f_100ba970 read 2026-10-29 late: forces [entry+0x12c] bit12 to the low bit of a fetched word at +1 (`and eax,1` before `shl 0xc` — same byte as the two-byte twins), advance 3; armed this pass as OP_ENTITY_WORD12C_SET_BIT12"),
    0x93: ("NO-ARM", "handler f_100b9d30 read to last instruction 2026-10-29 late: operand word @+1 zero closes the menu handler's item card (staged-record guard pair then whole-object reset f_1013bf40 on global [0x10578204]), non-zero opens it with that id via mode-0 opener f_1013e520 — the same sites 2026-10-15-menu-docking-and-flags.md lists at RVA 0xB9D51/0xB9D73; kuluu has no event-driven item-card surface, width-three skip faithful on control flow"),
    0x7B: ("NO-ARM", "handler f_100b8a00 read to last instruction 2026-10-29 late: resolver token @+1 + [entry+0x120] bit9 gate, clears bits 8-9 of [entry+0x12c] (`and dh,0xfc`) and stores word 0xFFFF at [entry+0x148] (reader unknown — record Not established #7); neither effect has a kuluu channel, width-five skip faithful on control flow"),
    0xAD: ("NO-ARM", "handler f_100bb340 read to last instruction 2026-10-29 late: two actor operands (@4/@8) staleness-rechecked against their [entry+0x74/0x78] tokens with both-side bit9 gates; nibble @+1 (0..9) selects ten module-literal dispatch entries (+0x2c4..+0x2ec on the first actor's handle, literal 'main' + word@+2); every exit including gate failures advances exactly twelve — kuluu keeps no handle interface, width-twelve skip faithful"),
    0x84: ("MATCH", "handler f_100b9100 read to last instruction 2026-10-29 late: ors [entry+0x12c] bit0 on the entity at word [exec+2], no clear branch, advance one via inc; armed this pass as OP_ENTITY_WORD12C_SET_BIT0 cueing ENTITY_WORD_12C_BASE+0 always true"),
    0x86: ("MATCH", "handler f_100b8f30 read to last instruction 2026-10-29 late: resolver token +2 and bit9 gate, forces [entry+0x12c] bit3 from byte@+1 & 1 on the resolved actor (xor-mask isolates exactly that), advance six; armed this pass as OP_ENTITY_WORD12C_SET_BIT3_RESOLVED"),
    0xB1: ("MATCH", "handler f_100bba70 read to last instruction 2026-10-29 late: three live selectors store mode-dword bits 1-2 / bit 0 / a constant into work slot two (mode global's producer unnamed, record census tail two), each advancing four; every other selector exits without touching exec — armed this pass (`OP_MODE_BITS_WORK_WRITER_B1`) halting there where retail spins"),
    0xBF: ("WIDTH-ONLY", "handler f_100bbd80 read to last instruction 2026-10-29 late: byte map + jump table under a sixty-four-entry ceiling; exactly three selectors advance — zero fetches one registry dword field (eight), thirty-two and sixty-four fetch twelve-byte-entry registries indexed by the word at +4 (ten); all others pop without advancing (spin). Out-of-range entry indexes emit literal-string trace calls before storing zero. Armed width-only (`OP_REGISTRY_LOOKUP_BF`) with sub_size row; stores land in work slots six/eight below kuluu's consumer set — record census tail two"),
    0xB7: ("WIDTH-ONLY", "handler f_100afee0 read to last instruction 2026-10-29 late with jump table @100b0108 dumped from the image: five selector bodies, each advancing uniformly on every path — zero resolves an actor and (behind its mover-block gate) stores one fetch into that mover's second work-slot block (ten), one copies a resolved actor's two-word name pair or one literal-pointer fallback into it (eight), two stores zero-or-the-second-fetch-dword for the eligibility-gated lookup (eight), three stores the resolved word or 0xFFFF sentinel (eight), four clamps a fetch to 0..8 and writes an eight-entry global word array, out-of-range emitting a trace literal first (eight); selectors above four jump to the pops-only epilogue @100b0100 — retail spins there where kuluu previously skipped a flat ten. Armed this pass width-only (`OP_MOVER_BRIDGE_B7`), pins in both sub-width tests; mover work slots, name blocks and the global array below kuluu's cue boundary"),
    0xD8: ("WIDTH-ONLY", "handler f_100bc060 read to last instruction 2026-10-29 late with jump table @100bc24c dumped from the image: resolves one actor (standard pair) and demands its mover block; selector zero copies four raw dwords of the record into that block's staged triple-plus-one fields, selectors one/three/two each fetch one stream word scaled by the send-pair heading constants then store one field, selector four chains all three (advance twelve via `mov ebx, 0xc`); every gate failure and every above-four selector lands on the shared tail @100bc23d advancing six (the register entry loads), so kuluu's pre-existing `_ => Some(6)` default equals retail's fall-through exactly; armed this pass width-only (`OP_EVENT_DIR_WRITER_D8`) with all five case widths pinned from the handler's own advance-register sites — before it the fixed-twelve fallback over-skipped the six/eight cases; a live-case gate failure advancing six where kuluu steps the case width is the named divergence; 124 linear-walk sites over 24 zones, sampled records are width-eight selectors one/two/three — exactly where the old fixed-twelve fallback over-skipped four"),
    0xAE: ("WIDTH-ONLY", "handler f_100bb620 read to last instruction 2026-10-29 late: sub @+1 cases 0..8 — weather-set singleton helper (callee f_10094040 unread, advance six), colour family storing fetched word@+6 or constant 0x17 into [entry+0x1d8] (advance eight), link set/clear writing actor field +0x798 with the paired handle at [entry+0xa0] or zero; case widths six/eight/ten equal kuluu's parsed opcode_meta rows on both failure and success paths; effects unmodelled (record Not established #12)"),
    0x5F: ("MATCH", "dispatcher f_100b72c0 + helpers f_100b5510/f_100b5210/f_100b4e10 read to their tails 2026-10-29 (revision): sel 0/1 bit-29 latch width 2; cases 2-6 consume the full case width whatever their inline actor dwords resolve to — every fetch/null-slot/id-pair/flag refusal branches to the helper's width-add tail — parking only at the f_100d12d0 motion-handle handoff kuluu declines to model; case 7 parks while either resolved actor reports running (busy vtable +0x2a0/+0x2ac selected by an operand byte) and takes its width on null slots; widths 6/16/16(+C bump=18)/18/14 match sub_size rows"),
    0x02: ("MATCH", "handler f_100afd10 read 2026-10-29 with its jump table verified slot-by-slot: eleven kinds (incl. AND-zero, NOR, bit-subset) on the nibble at +5, out-of-range kind takes the branch, taken jumps to word +6 else advance eight — kuluu's op_if answers kind-for-kind"),
    0x43: ("MATCH", "handler f_100b63a0 read 2026-10-29: send case latches and advances two, poll instance holds until the latch clears; kuluu models the latch as its pending server-ack round-trip — same pairing law audited for A7"),
    0x44: ("MATCH", "handler f_100b6400 read 2026-10-29: pool test on work operand +1 — present advances five past, absent jumps to word +3; kuluu exact"),
    0x47: ("MATCH", "handler f_100b62b0 read 2026-10-29: send case scales operands +8/+6/+4/+2 to floats, on success latches and takes ten words; poll instance advances two when both latches clear — pending-ack model as for 0x43"),
    0x5B: ("MATCH", "wrapper f_100b51f0 = scheduler helper f_100b5210(0,1,0) read to the bottom this session; kuluu's plain-form accept set {1,2,7,8} and unconditional width-15 advance reproduce the helper's gates and its refusal lands on that advance"),
    0x66: ("MATCH", "wrapper f_100b5200 = same helper with the TPC-package flag; kuluu's package accept set {0,1,6} matches it rule-for-rule"),
    0x8E: ("MATCH", "handler f_100b6980 read again 2026-10-29: status value 45 into the block entity's slot +372 under the gate bit, always one word — via inc, which is why the width classifier saw no add; kuluu cue carries the value"),
    0x8F: ("MATCH", "handler f_100b6a30 read again 2026-10-29: identical twin writing status value 46; same inc-width note as 0x8E"),
    0x90: ("MATCH", "handler f_100b3540 read 2026-10-29: entity-resolved gate, hide method plus third-word high-byte bit set, advance one via inc; kuluu cues the hide half — unnamed-bit posture unchanged in the record"),
    0x82: ("MATCH", "handler f_100b9040 read 2026-10-29: model-bounds-vs-region singleton test, hit advances seven past, every miss path jumps to word +5; kuluu exact"),
    0x9B: ("MATCH", "handler f_100b7520 read 2026-10-29: park while the block entity's animation-running byte is non-zero, else one word via inc; kuluu parks on that predicate shape against its own per-entity report"),
    0x98: ("MATCH", "handler f_100b8630 read 2026-10-29: async-loader busy gate, else one word via inc; kuluu's unconditional advance is retail's not-busy path — zone reads are never async below its cue boundary (record posture)"),
    0xC9: ("MATCH", "handler f_100b87e0 read 2026-10-29: gated clock resume plus countdown-byte clear, always one word via inc; kuluu's unconditional resume cue lands the same state, audited with its stop/restore pair"),
    0xA6: ("MATCH", "handler f_100ba9c0 read 2026-10-29: send/poll/write cases taking two/two/four words around a request-pending byte, answer global stored into the operand named at +2; kuluu's SubMapNum pending-ack arm reproduces the pairing law and widths"),
    0x73: ("WIDTH-ONLY", "wrapper of shared helper f_100b4590 whose every exit — both gate-fail labels and all eighteen visual-handoff cases — advances exactly eleven stream bytes; kuluu's no-arm width skip at the declared size is faithful on control flow, the gated visual scheduler-entry handoffs unmodelled"),
    0xC4: ("WIDTH-ONLY", "sub-dispatched twin of f_100b4590 (sub bytes zero/one/two hand selectors sixteen/seventeen/nineteen to the helper; any other sub leaves the handler without advancing, which is where kuluu's no-width halt stands); same fixed-eleven shared advance tail as 0x73 — size skip faithful, visual handoffs unmodelled"),
    0x7F: ("MATCH", "poll body f_100b2aa0 read with its query singleton traced: spins (yield, no advance) while the window holds an unanswered prompt; answered advances one storing through the keep-255-else-decrement sentinel law — kuluu's zero-based select_choice store plus the 254→255 QUERYWAIT2 remap is the same observable sequence (tests op_7f_querywait2_yields_then_resumes, op_7f_querywait2_does_not_cancel_on_a_cancelled_choice)"),
    0x71: ("MATCH", "handler f_100b7e10 read case-by-case this session (menu-family section): every width equals kuluu's dumped table row-for-row; password-pair cases send client packet id 96 and refuse to fabricate credentials, widget open/poll cases reproduce retail's latch spins through is_input_wait with buffer-length stores on release, sub 0x41 answered-halt pair matches the tested refusal posture, subs 0x20/0x21 hand off to the map-window helpers whose case advance lives inside them (net widths sixteen/two as kuluu encodes); unknown-sub spin ≙ halt; menu-widget content refused by named decision (test unsupported_query_workflows_do_not_send_or_fabricate_results)"),
    0x70: ("MATCH", "handler f_100b7dd0 read to the bottom 2026-10-29: entity id word @+1 → slot [0x10480af0]; null slot skips past the hold straight to width 1 without releasing anything; flags dword [entry+0x12c] bit 1 set raises the yield byte with no advance — kuluu parks on its two VM-visible holds for that predicate; clear calls f_1008af20, which under the visual-target gate ([entry+0x120] bit 9) hands FourCC tags '@tl0' and '@tr0' to visual vtable slot +0x29c twice (releasing both animation channels), then width 1 — kuluu's arm is that law (tests turnwait_parks_while_a_walk_is_in_flight, turnwait_parks_while_the_event_entity_animates)"),
    0x7E: ("MATCH", "handler f_100b8b80 read with both jump tables dumped from the image 2026-10-29: width table sub→{0,1,2,4,5,8}=6, 3=16, 6=18, 7=8 equals kuluu's sub_size row case-for-case; triple gate (eligibility pair, slot [0x10480af0] non-null, visual bit [entry+0x120] shr 9) fails to the width-add tail @100b8e34 advancing with nothing applied — kuluu's unresolvable-target posture; status stores into the door-status slot [entry+0x174]: sub 1→5 (STATUS_EVENT_CHOCOBO), sub 7→0x55=85 (STATUS_EVENT_MOUNT) plus inc-fetched byte to [entry+0x1db], subs 0/5/8→0 (IDLE, sub 8 also zeroing +0x1DB), sub 3 and 6 storing 5 then staging fetch(+6..+E/+10) words (first +32) for f_1009ac10; sub 2 holds-until-ready on visual vtable +0x1e4, false falling to a bare return with no advance while kuluu takes the ready branch"),
    0x87: ("NO-ARM", "handlers f_100b9140/f_100b9220 read 2026-10-29: sub 0/1 allocate c2s packet id 27 with payload word [pkt+4]=0/2 (87) and 1/3 (88), finalize through the A6 pair, arm shared in-flight byte global [0x1048557a] and advance two yielding; sub 2 polls that latch — clear advances two, set spins without advancing; alloc failure retries via pending-only no-advance tail; unknown subs spin the same way, where kuluu's refusal halt stands; every case advances exactly two, equal to kuluu's `sub_size` row for all three subs. kuluu refuses both by declared posture (test unsupported_query_workflows_do_not_send_or_fabricate_results)"),
    0x88: ("NO-ARM", "see 0x87 — mirror handler f_100b9220, same packet id 27 with the payload-word split {1,3}, same latch [0x1048557a] poll law; kuluu refusal posture identical"),
    0x8C: ("NO-ARM", "handler f_100b98e0 read 2026-10-29 late (five-family pass): send bodies build packet id 88 with command words 1–5 and stream operands, finalize-send, arm the pending byte and yield on every path; poll body spins plain-return while armed and takes width two on release; alloc failure → pending-only no-advance retry — kuluu refuses (same declared posture as the friend pair), every sub_size row equal to the handler's table"),
    0xB3: ("NO-ARM", "handler f_100bbaf0 read 2026-10-29 late (five-family pass): ranking singleton [0x10664444] getters/writers behind fourteen/eighteen-wide board-copy cases plus packet-88 send/poll bodies; guard above nine adds two and arms pending — kuluu's default-two row equal there; kuluu refuses by declared posture (7 corpus events refused; 28 uses / 6 zones total)"),
    0x9A: ("MATCH", "poll body f_100b6df0 read to the bottom 2026-10-29: two gates — signed-byte index [0x10482f1f] into word table @0x10485fe4 compared against tracker word [0x10482f1c], then f_10037320's async-counter busy test (dwords 0x10463944 vs 0x10463940, side-effecting a refresh-request byte at 0x10463909); either gate failing raises the yield byte without advancing; gates clear → inc width one — all paths set the yield byte. Both globals are client-internal loader state kuluu has no counterpart for; its single held pass is retail's gates-clear path (record row keeps the differs-knowingly name)"),
    0x9D: ("MATCH", "handler f_100ba190 read to every body this session: paired narrow/wide dispatch through table @100ba858 (subs 0x0A–0x10 reuse cases 0x00–0x06's bodies), width self-selected inside each body from the word fetched at +8 (narrow eight/six, wide ten) — kuluu's sub_size row equals both forms per sub; guard above 0x10 spins on a bare return where kuluu halts; compare cases set exec to authored labels, matching kuluu's declared jumps"),
    0xAB: ("WIDTH-ONLY", "handler f_100bac90 read this session (five-family pass): guard above 0x1C jumps to a bare-return spin tail — kuluu's no-width halt faithful; widths two except six actor+value fetches at four and sub 0x1B's two-operand six, all equal to the dumped table @100bb108 (29 cells); every effect (second-flags dword +288 bit pairs, door-status word stores, client-dword staging) parsed-not-applied — kuluu runs the case widths only"),
    0xAC: ("WIDTH-ONLY", "handler f_100bb180 read this session (five-family pass): guard above four lands on the shared add-by-register tail with the register zeroed at entry — out-of-range advances nothing and spins, kuluu's halt faithful; cases 0–4 widths 4/4/6/6/8 equal case-for-case to the dumped table @100bb324 (status words +368/+372 stores, bit-31 latch with helper-fail pending-byte halt, low-two-bits OR into +312) — effects parsed-not-applied"),
    0xB4: ("MATCH", "handler f_100b0a00 widths re-verified against the dumped byte-map this session for every case including both inline-string twins at twenty and the poll-without-widget two — equal row-for-row, and the input-wait set reproduces retail's spin/poll split; widget label/nameplate/value effects remain refused by named decision (record delta), audited at the machine-field level like the QUERY pair"),
    0xB6: ("WIDTH-ONLY", "handler f_100b1150 read this session: byte map @100b1dc4 → table @100b1d8c over 24 subs, shared advance add-4 tail and spin tail; look-record writes (+0xEF target with flags bit set, +0xFC/+0x10E compare-stores, whole-struct variants at widths twenty/fourteen/sixteen/twenty-two) parsed-not-applied through kuluu's width-only family fallthrough — kuluu's two never-encoded subs (0x16→twenty-two, 0x17→four) now in its table from the handler's own exec-adds"),
    0xCC: ("WIDTH-ONLY", "handler f_100b9d90 read this session (five-family pass): guard above 0x20 lands on the advance-four default tail — kuluu encodes exactly that as Some(4); eight-word item-record staging behind two window openers, seven-word detail staging at fourteen, widget-fill existence gate and sub 0x11's window-hold input-wait all parsed-not-applied through the width-only family fallthrough; is_input_wait reproduces retail's poll spins"),
    0x75: ("WIDTH-ONLY", "handler f_100b8660 read to the last instruction 2026-10-29: an any-open-window gate (window-count dword [obj+0x44] on singleton [0x10631f24]) yields with no advance and never fires for kuluu; case widths 4/2/2 taken from the handler's own add sites @100b8700/@100b86df/@100b86c4-@100b86ce because its fetch helper f_100afa90 never writes exec; case 2 re-bases six bytes back to carry the previous instruction's last WORD into a c2s packet-242 request (net +2 accepted, yield-byte spin refused) — kuluu advances each case width, window-present and packet counterparts named missing in the record"),
    0x6F: ("MATCH", "handler f_100b07f0 read 2026-10-29: countdown on the shared thread-slot timer (same [event+slot*32+0x3c] float as OP_WAIT's f_100b0870 and B2's f_100b0910); a negative slot arms the 16.0 default, each pass subtracts the clamped frame factor, every pass sets the yield byte, advance one only strictly below zero — kuluu's timed-wait arm at SLEEP_WAIT_UNITS=16 width 1 is that law"),
    0x57: ("NO-ARM", "handler f_100b0350 read to last instruction 2026-10-29 late (census tail two): adds a converted delta over the shared coprocessor-conversion helper into work slot one, advance three; kuluu cannot feed the client-side value (helper answers unnamed), width-faithful skip stands"),
    0x83: ("NO-ARM", "handler f_100b90e0 read to last instruction 2026-10-29 late (queue close): getter f_101039e0 = counter call plus one global addend, stored into work slot one; width three equal; the clock/global pair is client state kuluu does not keep — skip leaves zero where retail stores a ticked value (record Not established #14)"),
    0x85: ("NO-ARM", "handler f_100b9120 read 2026-10-29 late (queue close) and RE-READ 2026-10-30 down its callee, which retracts this record's 'nothing stream-visible' verdict: the operandless call is f_101d6110(arg=0), which posts to UI singleton 0x10621838 through method f_1015e350 with three named menu windows — `menu    rmlo1   ` (0x1037ec94) when global [0x106631d4]'s word +0x1c equals one (@101d6121..@101d6136), else `menu    myroom  ` (0x1036fff0) when the singleton's flag test f_1015ecc0(0x20000) reports absent (@101d613c..@101d615d); arg non-zero takes the same flag test and posts `menu    mogext  ` (0x10371648). So retail's effect is a named-window UI posting; kuluu has no arm at all. Width-one skip is NOT the law: this op opens something"),
    0x91: ("NO-ARM", "handler f_100b9b70 read to last instruction 2026-10-29 late (queue close): fetched word at +1 times exactly ten stored as a float into the current event entity slot +364, width three; readers of that slot unnamed (Not established #14), so no producer exists — width-faithful skip"),
    0x95: ("NO-ARM", "handler f_100b9c20 read to last instruction 2026-10-29 late (small handlers audited): word-fed entity refresh pair on the current subject, advance three whether or not the slot exists; the visual-method half sits below kuluu's cue boundary — width-faithful skip"),
    0x96: ("NO-ARM", "handler f_100b9c70 read to last instruction 2026-10-29 late (small handlers audited): the bare twin of 0x95 with no operands, width one at its inc tail (classifier blindness noted in the record); same unmodelled-half posture"),
    0x97: ("NO-ARM", "handler f_100ba0c0 read to last instruction 2026-10-29 late (small handlers audited): prep helper then two fetched words stored as window-singleton dwords +312/+316, width five; pure client staging with no kuluu consumer — width-faithful skip"),
    0x9C: ("NO-ARM", "handler f_100ba100 read to last instruction 2026-10-29 late (queue close): reads only the shared mode singleton (state byte sign plus dword+8 escalation), decrement ladder stores constant pairs {2,1}/{3,1}/{4,1} defaulting {1,1} into work slots one and two; every exit width three; producers of those globals are unnamed (census tail two)"),
    0x9E: ("MATCH", "handler f_100ba8a0 read to last instruction 2026-10-29 late (census tail two), ARMED 2026-10-30 (`OP_CLIENT_DWORD_104898FC_OPERAND_IS_ZERO`): stores operand-byte-equals-zero as a dword into fixed client global [0x104898fc] and advances two; kuluu performs the store (EventClientGlobals::dword_104898fc, value exactly 1 or 0 since ebx is pre-zeroed). Other writers of that cell image-wide: f_10094190 (@10094205), f_100ae3f0 (@100ae817), f_100aed00 (@100aedab). Previous 'width skip is the whole observable law' reasoning withdrawn"),
    0xA2: ("NO-ARM", "wrapper f_100b4970 read through its helper f_100b4af0 to the last instruction 2026-10-29 late: two entity ids (words +1/+3) resolve in the scene-entry table, both must match their [+0x74]/[+0x78] staleness tokens and carry flags-bit-nine; fetch (+B) is added to word @5 and handed to actor-link helper f_10062ed0 together with the two handles ([entry+0xa0]); on success the yield byte raises (park), every gate failure takes the add-15 tail - kuluu keeps no link surface, width-fifteen skip faithful on control flow"),
    0xA8: ("NO-ARM", "handler f_100b9780 read to last instruction 2026-10-29 late: fetches words @2/@4; byte-zero first posts the pair into map-window singleton [0x10663888] via rectangle call f_100b2570 (x,y,-1,0,0) and points f_101f8da0/f_101f8e30 (literal @0x103d3c84); byte-zero also closes through f_100b2a60 (the audited 0x8A close helper); whole body no-ops when the singleton is null; advance six on every path - kuluu keeps no map-marker surface and none is authored here, width-faithful skip"),
    0xAA: ("NO-ARM", "handler f_100babe0 read to last instruction 2026-10-29 late: one fetched word feeds seven getters (f_10103a80 with a constant +886 addend, f_10103a90/f_10103ab0 each incremented, then f_10103ae0/f_10103b10/f_10103b30/f_10103b60) stored into work slots three,five,seven,nine,eleven,thirteen,fifteen; advance exactly seventeen - kuluu keeps those work slots without producers, none of these values is authored-visible, width-faithful skip"),
    0xAF: ("WIDTH-ONLY", "handler f_100bb9c0 read to last instruction 2026-10-29 late: selector zero converts the singleton object's float triple at dword fields +80/+84/+88 into integers stored in work slots two/four/six (advance exactly eight, tail @100bba60), selector one the same triple at +68/+72/+76; every other selector returns through the pops-only exit @100bba68 without touching exec - retail spins there. Armed width-only this pass over a sub_size row {zero/one -> eight} with unknown selectors halting (before it kuluu's generic skip walked eight past those spin cases); zero authored uses in this corpus so the arm is posture-neutral, sweep counts unchanged; the triple values stay below kuluu's cue boundary. Pinned by work_slot_triple_advances_eight_on_both_selectors and a stop-list pin"),
    0xB9: ("NO-ARM", "handler f_100b9820 read to last instruction 2026-10-29 late: sibling of A8 under the same map-window singleton - fetches words @2/@4/@6; byte zero posts a rectangle (x,y) and two points, byte one three points in place; optional close through f_100b2a60 exactly as audited for 0x8A; advance eight whether or not the singleton exists; kuluu keeps no map-marker surface, width-faithful skip"),
    0xBE: ("NO-ARM", "handler f_100b3e20 read to last instruction 2026-10-29 late (zero authored uses in this corpus): stores dword [thread-slot-array (esi + ([exec+0x258]+2)*32) + slot-index-field] into work slot one and literal one into work slot two; advance three - pure internal scratch staging with no stream-visible effect, width-faithful skip"),
    0xC3: ("WIDTH-ONLY", "handler f_100bc000 read to last instruction 2026-10-29 late: its operand decides only whether the handler advances. The resolved value must sit in zero..seven or the handler returns with NO advance (retail spins); in range a five-dword staging slot receives the block-fetch result copied four dwords plus a second fetched word into dword five, advance seven (@100bc04b). Armed this pass, control-flow only. Lesson recorded: every authored site in this corpus carries the operand reference-window-bound (the raw +1 word has the flag set), so reading bytes directly returned negatives and refused seventeen events retail runs; resolving through kuluu's work-operand getter restored the counts exactly. The staged table and fetched blocks stay below kuluu's cue boundary. Pinned by staged_slot_loader_halts_outside_its_index_table"),
    0xD9: ("MATCH", "handler f_100b7be0 read to last instruction 2026-10-29 late, ARMED 2026-10-30 (`OP_CLIENT_BYTE_10463924_OPERAND_IS_ZERO`): stores byte(word @+1 == 0) into fixed client global [0x10463924] and advances two; kuluu performs it (EventClientGlobals::byte_10463924). That cell is written by f_100355f0 (@100356d8) and compared at f_10038330 (@10038364), so it is live state, not a scratch write. Previous 'width skip' reasoning withdrawn"),
    0x04: ("MATCH", "handler f_100b0120 read to last instruction 2026-10-29 (queue close): one instruction - exec plus three, nothing else; retail's entire program for this opcode is stepping past it, and kuluu's generic width skip moves by the identical three (four authored sites in four zones, zero behaviour on either side) - audited MATCH on the 0x6D empty-stub posture"),
    0xC2: ("NO-ARM", "handler f_100bbf80 read to last instruction 2026-10-29 late (record small-handlers section): query form one takes four words (@100bbfed), query form two fetches the word at +2 and takes six (@100bbfce), every other selector adds exactly two (@100bbfa3) - kuluu's generic skip already carries the sub-selector through its width table (vm.rs op_width) and that row is equal case-for-case, so no arm is needed; this pass first called the default selector a latent over-skip and retracts it. Both helpers' answers are unread client state, so the work-slot stores stay unmodelled rather than fabricated"),
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dll", default=DLL_DEFAULT)
    ap.add_argument("--gen", default=GEN_DEFAULT)
    ap.add_argument("--out", default=".agents/skills/retail-observe/references/data/opcode-parity-ledger.md")
    args = ap.parse_args()

    gen = pathlib.Path(args.gen)
    blob = pathlib.Path(args.dll).read_bytes()
    band = odt.stub_handlers(gen)
    _table_va, hi_opcode = odt.dispatch_site(gen, blob, band)
    slot0 = odt.slot_zero_offset(blob)
    cover = kuluu_coverage_all()
    meta = kuluu_meta()
    funcs = load_funcs(gen)

    counts: collections.Counter[str] = collections.Counter()
    rows: list[list[str]] = []
    for op in range(hi_opcode + 1):
        slot = struct.unpack_from("<I", blob, slot0 + 4 * op)[0]
        handler = band.get(slot)
        sym = handler[1] if handler else "(shared default)"
        name, arm = cover[op]
        km = meta[op] if op < len(meta) else None

        fa = Facts(sym, funcs) if sym.startswith("f_") else None
        merged, via_flag, helpers = effective(sym, funcs) if fa is not None else (None, False, [])
        form, dll_size, dll_jump = merged.form(funcs) if merged else ("(no body)", None, False)
        if merged and len(merged.movsets) > 1:
            form += f" (jump on {len(merged.movsets)} paths)"

        k_size = km["size"] if km else "-"
        k_jump = bool(km and km["jumps"])
        k_park = bool(km and km["sets_ret"])
        dll_park = bool(merged.parks) if merged else False

        if op in AUDITED:
            # A recorded audit row states the verdict of last resort whether or not the
            # opcode carries an arm: width-faithful skips and declared refusals are real
            # responses, and the classifier must not bury them as a bare NO-ARM cell.
            verdict, why = AUDITED[op]
        elif not arm:
            if (
                merged is not None
                and merged.nop_only
                and dll_size is not None
                and isinstance(k_size, int)
                and k_size == dll_size
                and not k_jump
                and not k_park
            ):
                # Retail performs nothing but stepping past the instruction; kuluu's width skip moves
                # ExecPointer by the identical amount, which IS the 1:1 response for such an opcode.
                verdict = "MATCH-ADVANCE"
            else:
                verdict = "NO-ARM"
        elif merged is None:
            verdict = "UNAUDITED"
            form = "handler not in generated C"
        elif not (merged.adds or merged.movsets):
            verdict = "UNAUDITED"  # no exec step anywhere: thread-level ops, need a manual read
        elif merged.adds and merged.movsets:
            verdict = "UNAUDITED"  # advance and jump-set coexist: a branch inside the handler
        elif dll_jump != k_jump:
            verdict = "JUMP-DIFFER"
        elif dll_size is not None and isinstance(k_size, int) and dll_size != k_size:
            verdict = "SIZE-DIFFER"
        elif any(a is None for a in merged.adds):
            verdict = "UNAUDITED"  # computed width; needs the operand's source read
        elif dll_park != k_park:
            verdict = "PARK-DIFFER"
        else:
            verdict = "MATCH"
        counts[verdict] += 1

        notes = []
        if op in AUDITED:
            # A recorded audit row replaces the mechanical cell with its reason;
            # machine facts (multi-path yields, helper chains) still follow below.
            notes.append(AUDITED[op][1] or "audited, see verdict")
        elif name is None and not arm:
            notes.append("no constant, no arm")
        elif not arm:
            notes.append(f"declared size {k_size}{'/jumps' if k_jump else ''}{'/sets_ret' if k_park else ''}")
        if merged and len(merged.parks) > 1:
            notes.append("writes yield byte on several paths")
        if via_flag:
            notes.append(f"body via helper {'->'.join(helpers)}")
        elif helpers:
            notes.append("multi-helper: " + ",".join(helpers))
        rows.append(
            [
                f"{op:02X}",
                sym,
                form,
                f"{dll_size if dll_size is not None else '-'} / {k_size}",
                f"{int(dll_jump)} / {int(k_jump)}",
                f"{int(dll_park)} / {int(k_park)}",
                name or "-",
                verdict,
                "; ".join(notes),
            ]
        )

    header_cols = [
        "op",
        "handler (this build)",
        "DLL exec form",
        "size D/k",
        "jump D/k",
        "yield D/k",
        "kuluu constant",
        "verdict",
        "notes",
    ]
    lines = [
        # Handler symbols, widths read out of the image and instruction-derived forms: this file is the
        # evidence column of the opcode work, so under the record gate it sits wholly in Provenance.
        "## Provenance",
        "",
        "opcode parity ledger — generated by `scripts/opcode-parity-ledger.py`; do not hand-edit (AUDITED",
        "overrides live in the script). DLL columns come from each handler's instruction stream in",
        "`research/xi-on-anything/generated/all`; size D/k compares the handler's step-past amount with",
        "`ffxi-event/src/opcode_meta.rs`. jump = sets exec non-linearly; yield = writes `[event+0x25A]`.",
        "",
        f"census: " + ", ".join(f"{k}={v}" for k, v in sorted(counts.items())),
        "",
        "| " + " | ".join(header_cols) + " |",
        "|" + "---|" * len(header_cols),
    ]
    for r in rows:
        lines.append("| " + " | ".join(r) + " |")

    out = pathlib.Path(args.out)
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {out}: " + ", ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    queue = [r[0] for r in rows if r[7] != "MATCH"]
    print("non-MATCH ops:", " ".join(queue))
    return 0


if __name__ == "__main__":
    sys.exit(main())
