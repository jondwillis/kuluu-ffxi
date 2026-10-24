# GetText string tables: files, formats and template escapes

Observed 2026-10-15. Scope: clean retail-2026-09 FFXiMain.dll read statically. No DAT files were present and no client or server was run, so file contents (entry texts, which escapes an entry uses, its declared parameter count) are unverified; everything below is what the client does with whatever the files hold. Covers how GetText(table, id) resolves for every table the client defines, both file formats behind them, the template escapes the client expands in tables 0, 8 and 0x0A, the arguments of the merit, job point and alter ego point yes/no prompts, and how the language row is chosen.

Table numbers are the client's own. Decimal 12, 13 and 14 in other records are the same tables as 0x0C, 0x0D and 0x0E.

## How a (table, id) resolves

Each table is either a d_msg table or an XISTRING table. GetText returns the entry's text, or no string when the id is past the table's entry count or the table has no file. A loaded d_msg table answers alone: an id past its count gives no string, with no fallback.

Template formatting is a separate step that a caller asks for. Tables 0, 8 and 0x0A are formatted with the escapes below at the call sites that request it. The same tables also serve plain lookups and printf-style use: the client hands table 0x0A entry 103 to sprintf as a format. So a caller chooses between plain, printf and template use. Nothing in the entry says which kind it is. GetText for table 0x0A must therefore give the formatter the raw bytes, escapes included.

## Files by table and language

The language columns are the client's four language rows (selection below). "same" means one file for every language.

| table | format | Japanese | English | German | French |
|---|---|---|---|---|---|
| 0 | XISTRING | 45 | 63 | not loaded | not loaded |
| 1 | d_msg | 55470 | 55470 | 55782 | 56202 |
| 2 | d_msg | 55468 same | | | |
| 3 | d_msg | 55467 | 55467 | 55776 | 56196 |
| 4 | d_msg | 55466 same | | | |
| 5 | d_msg | 55465 same | | | |
| 6 | d_msg | 55469 | 55469 | 55783 | 56203 |
| 7 | d_msg | 55526 | 55646 | 55766 | 56186 |
| 8 | XISTRING | 30 | 48 | 55767 | 56187 |
| 9 | d_msg | 55528 | 55648 | 55768 | 56188 |
| 0x0A | XISTRING | 32 | 50 | 55769 | 56189 |
| 0x0B | d_msg | 55530 | 55650 | 55770 | 56190 |
| 0x0C | d_msg | 55533 | 55653 | 55773 | 56193 |
| 0x0D | d_msg | 55532 | 55652 | 55772 | 56192 |
| 0x0E | d_msg | 55531 | 55651 | 55771 | 56191 |
| 0x0F | d_msg | 55539 | 55659 | 55779 | 56199 |
| 0x10 | d_msg | 55538 | 55658 | 55778 | 56198 |
| 0x11 | d_msg | 55540 | 55660 | 55780 | 56200 |
| 0x12 | d_msg | 55536 | 55467 | 55776 | 56196 |
| 0x13 | d_msg | 55534 | 55654 | 55774 | 56194 |
| 0x14 | d_msg | 55535 | 55465 | 55775 | 56195 |
| 0x15 | d_msg | 55535 | 55661 | 55781 | 56201 |
| 0x16 | d_msg | 55560 | 55680 | 55800 | 56220 |
| 0x17 | d_msg | 55546 | 55666 | 55786 | 56206 |
| 0x18 | d_msg | 55471 same | | | |
| 0x19 | d_msg | 55472 same | | | |
| 0x1A | d_msg | 55559 | 55679 | 55799 | 56219 |
| 0x1B | d_msg | 55525 | 55645 | 55765 | 56185 |
| 0x1C | none | never loaded by this build; GetText returns no string | | | |

Notes:
- No other table id resolves. All ids are file ids resolved through the install's VTABLE/FTABLE like any other.
- Where the English column repeats another table's file (0x12 = 55467, 0x14 = 55465), that is the client's own mapping.
- No direct GetText call in the client names table 0; it is reached through escape 0x81. For German and French without the English flag, table 0 is not loaded, so 0x81 sub-templates have no text there.
- kuluu already reads tables 7, 9, 0x0C, 0x0D and 0x0E correctly, as d_msg on the English row (`kuluu-render/src/hud/gettext.rs`, `ffxi-dat/src/menu_windows.rs`).
- Naming trap: `CAPTION_CATALOG_TABLE_11_FILE_ID` (55651) is the client's table 0x0E. The client's table 0x0B is 55650.

## Language row selection

The client has a language value and one flag. Both come from a settings block the PlayOnline host fills before the client starts; no code in the client writes them.

Language values: 0 Japanese, 1 English, 2 German, 3 French. The client's own language command maps the names "japanese", "english", "german" and "french" to these values.

Row choice:
- Japanese uses the Japanese row and English uses the English row. The flag does not affect either.
- German and French use their own rows when the flag is clear and the English row when it is set.
- The same rule picks the files for tables 0, 8 and 0x0A, and the grammar used by the item-name escape (German and French with the flag use the English rules).

The "second English row selected by a config bit" in earlier notes is the German row (55766, 55768, ...).

For an English install, use the English column, so table 0x0A is file 50. A Japanese install uses file 32.

## d_msg tables

These parse as kuluu reads them (`ffxi-dat/src/dmsg.rs` `DmsgTable`). GetText returns the entry's first value.

Client behaviour kuluu's reader does not check:
- The first value must be a string (its type word is 0); otherwise GetText returns no string.
- The u16 at offset 8 must read 1, which means little-endian. Any other value except 0x10 makes the client byte-swap the header and records.
- Outside the 0x10 layout, the u16 at offset 12 must be 3 and the u16 at offset 14 must be 0; otherwise the table stays unloaded and GetText gives no string.
- The value 0x10 at offset 8 selects a plain layout with no inversion:
  - 8-byte records start at offset 0x40, and record number (id × stride + value index) holds a u32 text offset from offset 0x40.
  - The stride is the u16 at offset 0x3E.
  - A record whose second u32 is 0 is empty.
- The byte inversion (XOR 0xFF) applies only when the u16 at offset 10 is 1.

## XISTRING tables (0, 8, 0x0A)

The client reads three things: the version word at offset 0x0A, the entry count at offset 0x24, and the index at offset 0x38. The other header fields below come from POLUtils `XIStringTable.cs` (vendored, Apache 2.0), which validates them. The client checks neither the magic nor anything else.

| offset | size | field | client |
|---|---|---|---|
| 0 | 10 | "XISTRING" padded with NUL | not read |
| 0x0A | u16 | version; 2 = 12-byte index entries | read |
| 0x0C | 20 | zero | not read |
| 0x20 | u32 | file size | not read |
| 0x24 | u32 | entry count N; ids at or past N give no string | read |
| 0x28 | u32 | index bytes, 12 × N | not read |
| 0x2C | u32 | text bytes | not read |
| 0x30 | 8 | unknown | not read |
| 0x38 | 12 × N | index | read |

Index entry i is the 12 bytes at offset 0x38 + 12 × i:
- u32 at byte 0: text offset, counted from the text area at 0x38 + 12 × N. The text is the NUL-terminated byte string there.
- s16 at byte 4: size. POLUtils uses it; the client does not.
- s16 at byte 6: the entry's numeric parameter count, used only by the template formatter. POLUtils calls this field "unknown, 1 may indicate a different encoding".
- Bytes 8..11: not read.

A version below 2 makes the client use 4-byte index entries: an s16 text offset counted from 0x38 + 4 × N, and no usable parameter count. POLUtils rejects such files. None is known.

Text is not inverted. Bytes are ASCII plus Shift-JIS pairs, and template entries also carry the escapes below.

## Template escapes

The formatter scans the entry from its start until it reaches a NUL byte. At each position it checks two things:
- A byte in 0x81..0x9F or 0xE0..0xFC followed by a byte in 0x40..0x7E or 0x80..0xFC is a Shift-JIS pair.
  - If the first byte of the pair is 0xFA, the pair starts an escape. The escape code is the next byte, and these three bytes are consumed. The pair's second byte carries nothing the client reads.
  - Otherwise the pair is copied as is.
- Any other byte is copied as is.

So escapes must be recognised before Shift-JIS decoding. A 0xFA that is not followed by a valid second byte is copied as a plain byte.

An operand is one of:
- `01 n`: numeric parameter number n & 0x7F, counted from 1.
- `02 v`: the value v & 0x7F.
- `03 lo hi`: (lo & 0x7F) + 128 × (hi & 0x7F).

A "u14" is the same two-byte form as type 3's lo and hi, but without the type byte. A jump length L is one signed byte.

| code | operands | effect |
|---|---|---|
| 0x81 | one byte (sub-template id, low 7 bits), one ignored byte, then k operands, k = that table-0 entry's parameter count | formats table 0 entry id with the k operand values as its numeric parameters and no string arguments, then appends it |
| 0x82 | operand s, u14 n, u14 end, then n × u14 case offsets | goes to case s when 0 ≤ s < n, else to end. Offsets count from the byte right after the code. The end is remembered for 0x8D |
| 0x83 | operands a, b | signed compare: sets equal, less (a < b) and greater (a > b). All three are clear when an entry starts |
| 0x84 | L | skips the L bytes after L |
| 0x85 / 0x86 | L | skip if equal / if not equal; otherwise continue after L |
| 0x87 / 0x88 | L | skip if greater / if less |
| 0x89 / 0x8A | L | skip if greater or equal / if less or equal |
| 0x8B | operand v | appends v in decimal: most significant digit first, no padding, no sign, no grouping |
| 0x8C | operand k | appends string argument k, counted from 0. A null string appends nothing |
| 0x8D | none | if the scan is still before the last 0x82's end, goes there |
| 0x8E | operands item, form, article | appends item id `item`'s name (rules below) |
| other | none | nothing appended |

0x8E name rules:
- English: form 1 gives the singular, and article 0 adds "a " in front. The article is "an " when the singular begins with one of a, i, e, u, o (either case). Form other than 1 gives the plural.
- Japanese: the plain name; form and article are ignored.
- German and French without the flag use their own rules (not traced).
- An unknown item appends nothing.
- Which item DAT name fields hold the singular and plural was not traced.

Argument binding for a formatted entry:
- The caller first gives a count of string arguments and that many strings; these are string arguments 0, 1, and so on.
- Then come numeric parameters 1, 2, and so on, as many as the entry's own parameter count (index byte 6). The caller supplies them in that order.
- An argument string is copied verbatim; escapes inside it are not expanded.
- printf conversions mean nothing to this formatter, so "%" is literal.

## Merit, job point and alter ego point prompts

All four are formatted from table 0x0A with one string argument. The output is limited to 255 bytes plus NUL.

| prompt | entry | string argument 0 | numeric parameter 1 | `merityn` title |
|---|---|---|---|---|
| merit raise | 80 | merit name | that merit's `next` from s2c 0x08C `merits[]` (u8) | table 0x0D entry 130 |
| merit lower | 81 | merit name | none supplied | table 0x0D entry 130 |
| job point spend | 116 | job point type name | that type's `next` from s2c 0x08D `points[]` (the 10-bit `next`) | none |
| alter ego point upgrade | 123 | upgrade name (alter ego strings entry 2 × upgrade id) | s2c 0x08E `Costs[upgrade id]` (u16) | none |

- The name sources and the open and refusal flows are in `2026-10-15-merit-and-job-point-windows.md` and `2026-10-15-unity-and-fp-windows.md`.
- The template decides where the name and cost appear. The only escape that prints a number is 0x8B, so the cost, wherever the template prints it, appears as plain decimal.
- kuluu's printf-style `format_template` in `gettext.rs` does not apply to these entries.

Where the text goes: `merityn` draws the formatted text in 0x80808080 at its window's left and top plus 4 units, word-wrapped to the window width minus 8, with lines 16 units apart. The line-break rule was not traced.

## Established vs not established

Established:
- The table-to-file map for every table the client defines, per language, and which tables are d_msg and which XISTRING.
- The XISTRING fields the client reads and their meaning, including the parameter count.
- The full escape and operand grammar of the template formatter, and the argument binding.
- The four prompts' entries, arguments and wire sources.
- The language rule and the meaning of the flag.
- Where `merityn` draws the text.

Not established:
- The texts of entries 80, 81, 116 and 123, which escapes they use, and their declared parameter counts. The client passes no number for entry 81, so a non-zero count there would read past the supplied arguments.
- Whether any shipped XISTRING file is below version 2.
- The German and French name grammar.
- Which item DAT fields the singular and plural come from.
- Where the PlayOnline host takes the language and the flag from.
- The line breaker's rules, and how the text draw treats bytes other than ASCII and Shift-JIS pairs.

## Provenance

All locations are RVAs in retail-2026-09 (image base VA 0x10000000, raw offset == RVA in FFXiMain.unpacked.dll). Static reading only; scripts and saved disassembly under the session scratchpad `reader_kit/gettext/` (callers.py, tables.py, fmtcallers.py, tablemap.py, fmt.asm, load2.asm, objs.asm, merityn.asm). No implementation writer may read these mechanics.

GetText RVA 0x152EF0 jumps to 0x152F20: linear search of the registry at RVA 0x371DA8 (28 × {u32 table, u32 object VA}); a found object that is loaded (0x257B10, [obj+4] != 0) answers through 0x257950 → 0x2578B0(id, 0); otherwise the raw slot array VA 0x10578548 (32 slots) entry [table] is passed to 0x1530C0. Registry: 0x1B→VA 0x105786C8, 7→0x10578688, 8→0x10578638, 9→0x10578658, 0xA→0x10578538, 1→0x105784B8, 0xB→0x105784A8, 0xE→0x10578508, 0xD→0x10578518, 0xC→0x10578698, 0x13→0x105784F8, 0x14→0x10578648, 0x15→0x10578528, 0x16→0x10578668, 0x12→0x105784D8, 0x10→0x105784E8, 0xF→0x105786B8, 0x11→0x105786A8, 3→0x10578628, 2→0x105785C8, 5→0x105785E8, 4→0x105785F8, 6→0x10578618, 0x1C→0x10578608, 0x17→0x105784C8, 0x18→0x105785D8, 0x19→0x10578678, 0x1A→0x10578498. Objects 0x10578638 (table 8) and 0x10578538 (table 0xA) are only constructed/destroyed (0x152880/0x1528A0, 0x152900/0x152920, ctor 0x257860, dtor 0x257840), never loaded; 0x10578608 (0x1C) is loaded only by 0x153ED0, which has no callers or absolute references.

Loaders: 0x153C10 (called from 0x10D79) first runs fixed loader 0x153B70 (0x153EF0(obj, file): 0xD8C9→VA 0x10669444 and 0xD8CA→0x10669454 (not GetText tables), 0xD8A9→table 5, 0xD8AA→4, 0xD8AF→0x18, 0xD8B0→0x19, 0xD8AC→2), then the raw tables by language at 0x153C1E..0x153C8D: JP 0x153AF0 = 0x152FF0(slot 0, 0x2D), (8, 0x1E), (0xA, 0x20) at 0x153AF7/0x153B07/0x153B17; EN 0x153B30 = (0, 0x3F), (8, 0x30), (0xA, 0x32) at 0x153B37/0x153B47/0x153B57; DE/FR without flag 0x153C68..0x153C88 = slots 8 and 0xA from row entries 8 and 0xA. 0x152FF0(slot, file) sizes 0x1C4AF0, allocates, reads 0x1C4740 on file manager VA 0x10662DCC, stores the buffer in the slot. Then language-row d_msg loads 0x153C90..0x153EBC as (row index → object → table): 6→0x1B, 7→7, 3→1, 4→3, 9→9, 0xB→0xB, 0xC→0xE, 0xD→0xD, 0xE→0xC, 2→6, 0xF→0x13, 0x10→0x14, 0x11→0x15, 0x12→0x16, 0x13→0x12, 0x15→0x10, 0x16→0xF, 0x17→0x11, 0x19→0x17, 0x3D→0x1A. Raw slots equal row entries 5 (slot 0), 8 and 0xA, matching the hard-coded EN/JP ids.

Rows: 0x25B560(index) forces language 1 when flag and language ∈ {2,3}, calls 0x25B4F0(language) (jump table RVA 0x25B550: 0 → row RVA 0x38AB08; 1 → RVA 0x38A548; 2 → flag ? 0x38A548 : RVA 0x38A388; 3 → flag ? 0x38A548 : RVA 0x38A708; stored at VA 0x10669464), returns row[index]; rows are 112 u32. Config object pointer VA 0x10456A28 (object VA 0x10456984): language [cfg+8], flag = byte [cfg+0x18] bit 7. Init 0x157E0 copies language from VA 0x104569D0 and sets bit 7 from bit 0 of VA 0x104569F4 (0x1584F, 0x1587A); both lie in the settings struct VA 0x104569A8 (+0x28, +0x4C) that COM method 0x157D0 (vtable data at 0xFB0 and RVA 0x329BB0) hands to the host; no in-DLL writer of either field. Language command 0x1D2A10 compares "japanese" RVA 0x37EB84 → 0, "english" 0x37EB7C → 1, "german" 0x37EB74 → 2, "french" 0x37EB6C → 3, else (value+1) mod 4.

d_msg object load 0x2579B0 (magic "d_msg" RVA 0x33CE74 via 0x257830): u16 +8 == 0x10 → data +0x40, no fix-up; +8 != 1 → swap 0x257310 (header), 0x2573C0 (pairs), 0x2573E0 (entries); require +0xC == 3 and +0xE == 0; XOR 0x2572E0(flag u16 +0xA, ptr, len) inverts only when flag == 1; table size +0x1C (pairs at +0x40, count +0x28), block size +0x20. Getter 0x2578B0 → 0x257870 (entry) → 0x2574F0(entry, 0): u16 count, pair {u32 offset, u32 type} at +4+8k, type must be 0, marker u16 1 at entry+offset, text +0x1C; layout 0x10 via 0x2577C0 (stride u16 +0x3E, empty when pair +4 == 0, default string VA [0x10389DAC]).

XISTRING reader 0x1530C0: u16 +0xA < 2 → 0x153090 (4-byte entries, s16 offset, base +0x38+4N); else u32 at +0x38+12i, base +0x38+12N; N = u32 +0x24. Parameter count 0x153100 = s16 at +0x3E+12i. Uncalled 0x153050 (slot setter) and 0x153070 (compares u32 +0x34). POLUtils `vendor/POLUtils/PlayOnline.FFXI/FileTypes/XIStringTable.cs` and `Things/XIStringTableEntry.cs` corroborate magic, version 2, N at +0x24, 12-byte entries, data base.

Formatter: variadic 0x153180(out, size, table, id, nStr, ...) and 0x153150 (nStr 0) → 0x153A40: GetText, copy nStr dwords into A (local, 16), paramcount(raw slot[table], id) dwords into B, call 0x153310(out, size, table, id, A, B). 0x153310: SJIS pair test 0x1532F0 → 0x1532B0 (lead 0x81..0x9F/0xE0..0xFC, trail 0x40..0x7E/0x80..0xFC); escape test 0x153130 (byte == 0xFA); code at p+2, dispatch RVA 0x1539F4 (code - 0x81, 14 entries): 0x15340D sub-template (id = byte & 0x7F, +2 bytes, k from raw slot 0 via 0x153100, recursive 0x153310(table 0, A = NULL, B = parsed values)); 0x153588 switch (end stored in local 0x64); 0x153499 compare (frame bytes +1 equal, +2 greater, +3 less, cleared at entry); 0x15353A jump; 0x153517 JE; 0x15351D JNE; 0x153532 JG; 0x153547 JL; 0x15355C JGE; 0x153572 JLE; 0x153604 number → itoa 0x153230; 0x15365A string A[k] (strncpy 0x312050); 0x1535F1 end-case; 0x1536E1 item name: lookup 0x15BD40 (item cache VA 0x10621808, 0x15B9E0, name object 0x15B270 [item+0x14CC]); language table RVA 0x153A2C: JP 0x153770 (name vtable +4), EN 0x15378F (+8 singular, +0x10 plural, vowel test 0x258990 over "aieuoAIUEO" RVA 0x389EC0, "a " RVA 0x371E88, "an " RVA 0x371E8C), DE 0x153816, FR 0x15389D; flag override at 0x153738..0x153769. Operands 0x1531D0 (type 1: B[(b&0x7F)-1]; 2: b&0x7F; 3: u14 via 0x1531B0).

Formatter callers: tables 0xA and 8 only (0x23F628 table 8 id 0x4E). Table 0xA printf use: 0x79DE2 GetText(0xA, 0x67) → sprintf 0x311FF1 (CRT, _IOSTRG 0x42). Kuluu's 0x196ED1 citation in menu_windows.rs is GetText(0xB, 0x8F).

Prompts (output VA 0x105782E0, size 0x100, add esp 0x1C = one number, 0x18 = none): 0x149BE7 id 0x50 (name 0x161070, number byte +2 of record 0x160FB0); 0x149CB1 id 0x51 (name only); 0x14B5D7 id 0x74 (name 0x161250, number u16 >> 6 from 0x1612D0; s2c 0x08D handler 0xE4FD0 stores level | next << 6 at game +0x2D010); 0x14C14F id 0x7B (name 0x161720 = string 2n, cost 0x1617B0 = u16 game +0x2D638 + 2n, copied from packet +0x28 by s2c 0x08E handler 0xE5140). merityn query 0x210E30 stores text (+0x1C) and title (+0x34 → 0x150CC0); merityn handler vtable RVA 0x33BC30, draw hook 0x211050: rect 0x118F70, text box 0x156C30(x0+4, y0+4, x1-x0-8, text, 0, 0x80808080), line step 0x10 + arg, breaker 0x25B6C0.

LSB headers used: `vendor/server/src/map/packets/s2c/0x08c_merit.h` (merit_t next), `vendor/server/src/map/packets/s2c/0x08d_job_points.h` (jobpoint_t next:10, level:6), `vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h` (Costs[32] at packet 0x28).
