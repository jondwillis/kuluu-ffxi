# Inspecting the installed retail client

Use this route when the question needs the original computation or binary
layout. It is not a requirement to disassemble every vanilla change.

## Locate the relevant build and data

Installs live in the user's registry, never in the checkout: `cargo run -q -p
kuluu -- install list` names each one with its KNOWN_CLIENTS row, and `cargo
run -q -p kuluu -- install path NAME` prints the DAT root to inspect (the
`FFXiMain.dll` beside `VTABLE.DAT`). Inspect only the needed paths; that installation can
also contain account configuration and logs unrelated to the investigation.
`FFXiMain.dll` contains client logic and lookup tables. Resolve model DAT IDs
through the installation's VTABLE/FTABLE and existing `ffxi-dat` readers rather
than guessing ROM paths. `rg` normally ignores this install, so use an explicit
path with `--no-ignore` when searching it.

Search the relevant policy in `research/XIClient/src/XIClient/source/` to find
candidate callers and data structures. XIM and xi-tools references can supply search
terms. Preserve the distinction between a community hypothesis and a rule
independently confirmed in the installed binary.

## Packed FFXiMain.dll

Some builds store compressed code in a `POL1` PE section while `.text` has no
raw bytes. Inspect the actual PE section table before disassembly. The local
`research/xi-tools/docs/ffximain/ffximain.md` describes the LZSS decoder and
the `xi dll ffximain unpack` tool; read it as hypotheses/tools, not universal
offsets or lengths.

- Inspect the installed unpacker stub and section sizes rather than assuming
  the example build matches. Bound decoding to the actual destination size and
  reject invalid back-references or truncated output.
- Prefer a raw decompressed `.text` dump for analysis. Its virtual start is
  **ImageBase + .text.VirtualAddress**, not ImageBase alone. Distinguish file
  offset, RVA and VA in every citation.
- Do not run or replace the game's DLL with an analysis artifact. A PE whose
  `.text` raw size was zero cannot safely be reconstructed by just writing the
  code at its old raw pointer: another section may occupy that file range.
  Preserve the installed binary; put dumps in an ignored artifact directory or
  temporary directory.
- Use an available disassembler (for example Capstone, Ghidra or IDA) and
  confirm instruction boundaries and cross-references. A matching byte pattern
  alone is not a function identification. Respect normal tool-install and
  execution permissions; absence of a preferred tool is not a reason to claim
  the binary was verified.

## Make the conclusion reproducible

Record the input path, SHA-256, image base and relevant section RVA; include the
function addresses and how they were identified. For a DAT measurement, record
the file ID, resolved path, resource/locator index, raw value and coordinate
conversion. Scope addresses and measurements to that build: name the
`KNOWN_CLIENTS` row (`ffxi-dat/src/client_profile.rs`) when the build is one of
those, or the DLL SHA-256 (twelve hex digits or more) when it is not, and say
whether each address is an RVA or a VA. `scripts/checks.sh comments` enforces
this scoping on any comment carrying an FFXiMain.dll address.

In the observation record, all of that goes under a `## Provenance` heading.
The sections above it state the rule in interop terms only: what retail does
over DAT fields, wire fields and the screen, never where in the image it does
it. `scripts/checks.sh records` hard-fails a record that carries an address,
decompiler name, register sequence, vtable slot or `+0x` field offset outside
that section. The implementer reads the spec sections in a separate session;
the Provenance section exists so the finding can be re-checked on another
build, not so it can be ported.

The unpack recipe is proven on both `KNOWN_CLIENTS` rows: `research/xi-tools`
`uv run xi dll ffximain unpack` (or an independent LZSS decoder bounded to
`.text` `VirtualSize`) produces the raw `.text` dump, whose first byte is VA
`0x10001000` (RVA `0x1000`). Disassemble it ephemerally with capstone; nothing
is tracked in the tree, so write the one-liner on the spot:

```bash
uv run --with capstone python3 -c '
import sys, capstone
dump, va, n = sys.argv[1], int(sys.argv[2], 0), int(sys.argv[3], 0)
data = open(dump, "rb").read()[va - 0x10001000:][:n]
for i in capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32).disasm(data, va):
    print(f"{i.address:08X}  {i.bytes.hex():<20}  {i.mnemonic} {i.op_str}")
' <dump> <VA> <nbytes>
```

Install nothing project-permanent for a one-off read. The unpacked `.text`
SHA-256 per row: `horizonxi-2023`
`f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527`;
`retail-2026-09`
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.

Follow the call chain far enough to establish the actual inputs and exceptions.
For example, the nameplate investigation on `kuluu-81r8` found that the name
caller defaults to locator 2 and the model accessor special-cases that ID using
static translation and model scale. Reading only the general animated locator
helper would have produced a plausible but wrong fix. Consult that bead for
its build-specific evidence; do not reuse its addresses on another build.

Keep binaries, decompressed code, raw assets and screenshots out of git.
Commit independently written implementation and concise evidence citations,
not copied third-party source. A binary-derived rule still needs comparison
with the resulting Kuluu behavior before claiming runtime parity.
