# ffxi_disassembly

Static-analysis scanner suite for `FFXiMain.dll` — the retail oracle tooling named in
`AGENTS.md`. Python + capstone over a locally installed binary (PE32 i386, ImageBase
0x10000000). Read-only: nothing here patches or redistributes anything.

Findings live in `docs/` next to the tools; every script prints markdown meant to be
pasted into them. The build analyzed so far is TDS 0x6A7297F5 (PhoenixXI install); RVAs
are only valid against one build, so name the `KNOWN_CLIENTS` row from
`ffxi-dat/src/client_profile.rs` beside anything verified on it.

## Setup

```bash
pip install -r requirements.txt        # pefile, capstone
# Windows note: `python` may resolve to a project venv; use the system python.
```

The first script that needs disassembly runs a linear sweep of `.text` with capstone
(a few minutes) and caches it in `.cache/`. Every later run reuses the cache. Delete
`.cache/` when you swap in a different FFXiMain.dll build.

## Pointing at a DLL

Every script takes the install directory or DLL path as its first argument. In this repo
the retail install is registered, not assumed:

```bash
kuluu install list                 # named installs
kuluu install path <NAME>          # prints the DAT root; FFXiMain.dll sits beside VTABLE.DAT
```

## Tools

| Tool | What it does |
|---|---|
| `common.py` | Shared plumbing: PE load, `pol1_decode` (the bit-packed LZSS that unpacks `.text` on packed builds), cached linear capstone sweep of `.text`, `KNOWN_RVAS` anchors. |
| `p0_modmap.py` | Module map: PE32 i386 confirmation for every exe/dll, image bases, sections, imports/exports, version strings. Records the ImageBase/TimeDateStamp/section table every RVA depends on. |
| `p1_anchors.py` | Maps known RVAs to sections, dumps vtable slot lists, finds constructors and `ini`/`init` literals, class-name and `ROM/` strings, ActionTimer2 sites. |
| `p2_handler.py` | Scores every heuristic function by ten signals; prints top candidates with hit sites, callers, full disassembly of the top three. |
| `p3_gates.py` | Gate/predicate pass (entity-type gates and similar branch scans). |
| `p7_event_vm.py` | Event-VM pass: pattern-searches every XiEvents byte pattern (wildcards `??`) against the cached `.text` sweep; name -> hit RVA -> func start. A miss is the fallback to immediate anchors. |
| `p8_jumptable.py` | Dumps the `ExecProg` switch jump table as opcode -> entry VA -> thunk RVA -> handler RVA. Source of `docs/event_opcode_table.md`. |
| `p9_zone_scene.py` | Lists every DAT whose parsed scheduler routines reference movN / exNN stage names; file ids resolved through VTABLE/FTABLE. |
| `p10_combat_tags.py` | Combat tag census pass. |
| `p11_pkt_tables.py` | Packet table dumps. |
| `probe_tpc_files.py` | Tpc cross-check: resolves the four-band A/B file ids through VTABLE/FTABLE and lists which of tlk0 / thk1 / kka0 each mapped DAT carries. |
| `scene_dat_parse.py` | Parses zone scene DATs (scheduler routines, stage names) for the p9/probe workflows. |
| `dat_routines.py` | Shared DAT/scheduler-routine parsing used by the event-VM tools. |
| `ffxi_dat_find.py` | Two install lookups: `resolve <file id...>` (VTABLE/FTABLE -> ROM path) and `scan-tag <fourcc...>` (which DATs carry a scheduler chunk with those names). No build needed; `FFXI_DAT_PATH` or `--root`. |
| `assemble_event_evidence.py` | Rebuilds the raw evidence appendix (`docs/event_evidence.md`) from the local raw dumps. Extend its section lists when adding new dumps. |
| `xref.py` | Cross-reference: `--to 0xRVA` (callers), `--from 0xRVA` (callees), `--imm 0x...` (who uses an immediate / on-disk VA / fourcc), `--disp 0x11E --size 2` (who touches a displacement), `--tree 0xRVA --depth N`. |
| `disasm.py` | Disassembly: `--func 0xRVA` (whole heuristic function, annotated), `--rva/--len`, `--va 0x04DF0F40` (convert a runtime VA, base 0x04AC0000), `--bytes 0x32BB38 --len 0x100` (hex/dword dump of vtables/tables). Annotations: known-vtable VAs, fourcc immediates, plan offsets. |

## Examples

```bash
python disasm.py "<install>\FFXiMain.dll" --func 0xBC27C      # whole annotated function
python xref.py   "<install>\FFXiMain.dll" --to 0xD2120        # who calls it
python p8_jumptable.py "<install>" > out_p8.md                # ExecProg opcode table
```

## Findings index (`docs/`)

| File | What |
|---|---|
| `mob_animation.md` | Mob animation driver: synthesis, reference tables (RVAs, entity/actor layout, RenderFlags bits, stage ops), findings F1-F58, open items, kuluu conclusions |
| `event_vm.md` | Event VM and cutscenes: struct layouts, motion resource readers, request stack, wait predicates, GetActorIndex, zone scene DAT, camera, Type byte; findings E1-E20 |
| `event_opcode_table.md` | ExecProg jump table: opcode -> thunk -> handler RVA, with XiEvents names and width notes |
| `camera.md` | Event camera control: 0x46 DEFCAMERA case decode, camera manager object, work-slot -> camera scaling, look-at opcodes, 0x47 position update, 0x38 local mode, focal; findings C1-C11 |
| `ui.md` | Event UI/HUD and dialog control: HUD hide/unhide, 0x6A sound volume, cancel/ESC flag triad, 0x20 input lock, dialog create/wait-select, menu option masks, chat opcodes, string input, hide flags, 0x43 report, 0xB5 name; findings U1-U16 |
| `tpc_package_table.md` | Opcode 0x66 Tpc motion package -> A/B DAT file id rule (four bands) with worked examples |
| `mob_evidence_1_modmap_anchors.md` | §A module map, POL1 entry stub; §B vtable slots, ctor sites, fourcc literals (F24-F27) |
| `mob_evidence_2_entity_update.md` | §C per-entity update routine: PopEffect, create dispatch, flush wrapper, xrefs (F28, F34, F35) |
| `mob_evidence_3_handler_destroy_live_dat.md` | §D name resolvers / state switch; §E 0x0E handler; §F destroy path + xrefs; §G live sessions (routine records, combat); §H on-disk DAT dumps (F29-F34, F46-F55) |
| `event_evidence.md` | §A-§M raw dumps behind E1-E20 |

## Conventions

- **Addresses are RVAs** relative to `FFXiMain.dll` ImageBase **0x10000000** (on-disk VA = 0x10000000
  + RVA). Disassembly dumps print on-disk VAs (0x10xxxxxx). The DLL has no dynamic-base bit, but
  runtime bases differ per session; convert with `RVA = VA - base` (each log records its base).
- **`.text` is POL1-packed on disk** (rawsize 0): bit-packed LZSS in the POL1 section, unpacked by the
  entry stub at load. Every static scan runs on the decoded image via `common.py:pol1_decode`; RVAs are
  unaffected. `.rdata`/`.data` are not packed.
- **Hot functions are entered mid-function** and reached through vtables and thunk tables, so
  `xref.py --to <func start>` often returns 0. Use `--to` on the real entry address, `--imm <vtable VA>`
  for constructors, and `--disp <offset>` for field consumers.
- **Packet offsets vs struct offsets.** s2c body offsets (status +0x1C, animationsub +0x26) are not
  entity-struct offsets; in the handler `esi` is header-inclusive (body = esi+4).
- **Evidence tiers:** `[local]` = verified against this install's binary or memory; `[web]` = from a
  public source, unverified here; `[external]` = SE's own code via the PS2 decompile or a reimplementation.
  A web fact promoted to local gets a new entry; tiers are never edited in place.
- **Numbering:** mob pass findings are F<n>, event pass E<n>. Evidence sections are cited as
  "mob_evidence_3 §G.1" / "evidence §K". Every finding carries RVA + evidence pointer; nothing
  lives only in chat.

## Raw source map (local working dumps, untracked)

Scanner outputs land in local untracked dump folders (`out/` Phase 0, `out2/` mob pass,
`out3/` event pass, `out4/` camera + UI handler dumps); they are kept for re-runs and never
tracked. The evidence docs embed copies of the relevant outputs:

| Source | Reproduced in |
|---|---|
| `out/p0.utf8.md`, `out/entrystub.md` | mob_evidence_1 §A |
| `out2/p1.md` | mob_evidence_1 §B, mob_evidence_2 §C.4 |
| `out2/d_*.md`, `x_*.md` (per-function dumps and xrefs) | mob_evidence_2 §C, mob_evidence_3 §D-§F |
| wormwatch session logs | mob_evidence_3 §G |
| `dat_routines.py` on 5.DAT / 11.DAT / 32.DAT; `out2/datdump.zip` | mob_evidence_3 §H; F55 |
| `out3/*` (`p7.md`, `p8.md`, `d_*.md`, `x_*.md`, `probe_*.md`, `p9_zone_scene_scan.md`) | event_evidence §A-§M |
| `out4/d_*.md` (camera + UI handler dumps) | camera.md C1-C11, ui.md U1-U16 |
