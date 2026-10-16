# Reading FFXiMain.dll through REA

REA is a pinned Ghidra reader served over MCP, installed locally in `vendor/rea`
(pins and install: [vendor/README.md](../../../../vendor/README.md)). It is the
Ghidra route of [binary inspection](binary-inspection.md): it reads the file and
never runs anything. Everything that page says about builds, citations and the
reader/writer split still applies.

The `mcp__rea__*` tools exist only in a session started with
`--mcp-config vendor/rea/rea.mcp.json --settings vendor/rea/rea-loop-settings.json`
(absolute paths in `vendor/rea/INSTALL.md`). Without them, use the capstone recipe
in binary inspection.

## Target

`vendor/rea/targets/FFXiMain.unpacked.dll`: the full unpacked PE of FFXiMain.dll
`retail-2026-09` (TDS 0x6A995428, image base 0x10000000, file offset == RVA). It is
not the raw `.text` dump from xi-tools; Ghidra needs the PE. `open_binary` rejects
relative paths, so pass the absolute path printed in `vendor/rea/INSTALL.md`.

## Start of a session (once, before any other REA call)

1. `mcp__rea__open_binary` `{"path": "<absolute target path>", "provider_id": "ghidra"}`
2. `mcp__rea__binary_overview` `{}`

The first query makes Ghidra import and analyse the whole DLL. That takes minutes
(about 3 on 2 CPUs; this machine's time is in `vendor/rea/INSTALL.md`). Wait for it
and don't cancel it: a cancelled query throws the analysis away and the next query
starts over. After that, queries answer in well under a second.

If it fails or times out, call `mcp__rea__close_binary` `{}` and repeat steps 1 and 2
once. If it fails again, carry on with the capstone route and say so.

## Addresses

- REA answers in VAs; citations name the build and say RVA or VA.
  **RVA = VA minus 0x10000000.** OpenMenu is VA 0x1015E350 = RVA 0x15E350 in
  `retail-2026-09`.
- Pass addresses as 0x-prefixed hex VA strings: `"0x1015E350"`. Answers use
  lowercase hex.
- `FUN_1015e350`, `DAT_...`, `param_1`, `iVar2` are Ghidra placeholders, not names.
  Never record them as names.

## Which tool

| Question | Tool | Arguments |
| --- | --- | --- |
| What does this function do | `procedure_pseudo_code` | `{"procedure": "0x1015E350"}` |
| Exact instructions, offsets, constants | `procedure_assembly` | `{"procedure": "0x1015E350"}` |
| Who calls this function | `procedure_callers` | `{"procedure": "0x1015E350"}` |
| What this function calls | `procedure_callees` | `{"procedure": "0x1015E350"}` |
| Who touches an address (table, global, string, function) | `xrefs` | `{"address": "0x1036FF98"}` |
| References to a name | `find_xrefs_to_name` | `{"name": "..."}` |
| Find text in the DLL | `search_strings` | `{"pattern": "...", "mode": "literal"}` |
| Find functions by name | `search_procedures` | `{"pattern": "...", "mode": "literal"}` |
| Which function holds an address | `resolve_containing_procedure` | `{"address": "0x1014E7E1"}` |
| Raw bytes of a table | `read_bytes` | `{"address": "0x1036FF98", "length": 96}` |

The settings file exposes only these 13 tools. The rest of REA's 170 are removed
from the model's context; some (`analyze_function`, `binary_session`,
`procedure_references`, `get_call_graph`) return megabytes for one call on this DLL.

Every answer is JSON with about 5,000 characters of evidence header. The useful part
is `normalized_result` (the decompiled C, the assembly, the reference list).
`limitations` says what Ghidra could not establish; read it before trusting a gap.
Keep questions narrow: one function, one address, `read_bytes` lengths of 512 or less.

## Evidence

- Ghidra's C is a reading aid, not proof. Confirm a number, offset, flag or table in
  `procedure_assembly` or `read_bytes` before stating it as verified.
- Record findings the way [retail grounding](../SKILL.md) says: the rule in interop
  terms, with addresses, decompiler names and offsets only under `## Provenance`.

## End of a session

`mcp__rea__close_binary` `{}`
