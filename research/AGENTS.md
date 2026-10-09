# research/

`CLAUDE.md` and `README.md` here are symlinks to this file, so the tier list
below loads automatically for any AGENTS.md-aware tool working in this
directory, and still renders as the directory README for humans.

Read-only third-party material used while re-implementing FFXI client
behavior. **Nothing under here is redistributed by this repo** — it is either
a submodule pointer (URL + commit, no upstream bytes committed) or fetched on
demand and gitignored.

Treat everything here as **reference only**: study behavior and re-express it
in our own code. Do not copy, paste, or link third-party source into the
workspace crates.

## Contents

- `XiEvents/`, `XiPackets/` — atom0s's submodule pointers cited in source
  comments. XiEvents self-dates to the Feb. 28, 2022 retail client (older than
  every `KNOWN_CLIENTS` row); XiPackets tracks retail with a lag and is current
  through the early-2026 Alter Ego points packets (`0x00C1`/`0x008E`), with no
  DLL hash to anchor it. Deinitialized by default; populate on demand
  with `git submodule update --init research/<name>`. See *Which reference for
  what* below before trusting any of them for bit-level format details.
- `Phoenix/`, `xim/` — **not** submodules. Both upstreams are private or
  git-only, so they are git-ignored and you clone them here yourself. Every
  `Phoenix/…` citation in this tree assumes such a local clone; absent one,
  the path simply won't exist.
- `xi-model-viewer/` — [xi-model-viewer](https://github.com/vekien/xi-model-viewer),
  a Tauri/WebGL2 FFXI asset browser (zones, NPCs, PCs, spell effects,
  textures, audio) with GPU skinning. GPL-3. Reference for DAT parsing,
  skeleton posing, and zone/weather rendering. The successor of both
  cexi-viewer and voliathon's AltanaViewer, which are archived upstream.
- `xi-tools/` — [xi-tools](https://github.com/vekien/xi-tools), the DAT
  editing CLI and the community docs of FFXI's internal formats under
  `docs/` (DAT, animation, zone mesh, event bytecode, audio, VFX,
  FFXiMain unpacking). Successor of cexi-docs. Format cross-reference for
  `ffxi-dat` / `ffxi-audio`.
- `XIClient/` — [XIClient](https://gitlab.com/Aenge/XIClient), a from-scratch
  playable C++ FFXI client (no license — all rights reserved). Reference for
  client architecture and vanilla behavior only.
- `xim/` — the XIM browser FFXI client (**gitignored**, fetched locally). See
  below.

## Which reference for what

These sources are **not equally authoritative**. When they disagree, prefer
the higher tier:

1. **Retail itself** — the disassembled FFXiMain/POL `.text` and live
   observation (the `retail-observe` skill) are the oracle. Bit-level
   questions (field widths, masks, flags) are settled here, nowhere else.
   Build: the `KNOWN_CLIENTS` rows in `ffxi-dat/src/client_profile.rs` -
   `retail-2026-09` (patch `30260904_1`, the oracle) and `horizonxi-2023`
   (`30230905_0`, the HorizonXI pin); name the row, not a date, beside
   anything verified on it.
2. **`XIClient/`** — disassembly-grounded; the best community reference for
   **bit-level format accuracy** (field widths, in-memory-only bits). No
   license: read-only. Its reconstructions remain community evidence; cite them
   as such until corroborated by the relevant retail binary or observation.
   Its value is not only field widths: it carries retail's
   runtime *policies* named and intact, so prefer it over XIM whenever the
   question is "what exactly does retail do here", not just "what does the
   struct look like". `World/Zone/Terrain/` is the worked example — retail's
   ray queries are one template over the MZB collision grid, specialised by
   policy (`BacksideCullingPolicy` for movement, `DoubleSidedSkipPolicy` for
   the chase camera), which settles floor-vs-ceiling and camera-skip questions
   that XIM only approximates.
   Build: none pinned - it takes its version from the install's `patch.ver`
   and tracks live retail as a moving target (`GC_ZONE` and the DMsg v16
   loader were refreshed through 2026-05); the pin here (2026-07-28) falls
   between `horizonxi-2023` and `retail-2026-09`, so confirm a layout against
   the row you are on.
3. **`Phoenix/`** — server-side divergence signal for wire-protocol
   questions (LSB under `vendor/` stays authoritative for runtime). Not
   vendored; needs a local clone, so treat a missing path as "unavailable",
   not "no divergence".
   Build: unpinned - whatever commit your local clone is at; record it in the
   bead that cites it.
4. **`xi-tools/docs/`** — community format docs (DAT, animation, zone mesh,
   event bytecode, audio, VFX). Useful cross-reference for `ffxi-dat` /
   `ffxi-audio` work, but AI-assisted: treat claims as hypotheses and
   verify against tier 1–2 before baking values into the crates.
   Build: CatsEyeXI's client (`docs/HANDOVER.md`), a retail-lineage
   `FFXiMain.dll` newer than `horizonxi-2023` and older than `retail-2026-09`
   (`.text` `0x32716E` against `0x3230BE` / `0x3275EE`), so its VAs match
   neither `KNOWN_CLIENTS` row.
5. **`xi-model-viewer/`** — rendering and asset-pipeline reference: WebGL2 GPU
   skinning, zone time-of-day/weather, BGW/SPW playback. Most useful for
   `kuluu-render` materials and `ffxi-actor` posing.
   Build: v1.6.0; it ships no client and reads whatever install it is pointed
   at (the author's is CatsEyeXI, with lists baked by xi-tools).
6. **`xim/`** — lookup aid only, not evidence of retail behavior or a source
   of constants. Use it to locate a question, then establish the answer from
   applicable primary evidence. This applies to animation, effects, timing and
   camera policy as well as binary layouts. Existing XIM-derived code, tests,
   beads and records remain hypotheses until independently corroborated.
   A disagreement with XIM cannot by itself justify reverting DLL/DAT-grounded
   work or filing a parity defect. Follow the
   [source-conflict procedure](../.agents/skills/retail-grounding/SKILL.md#resolve-conflicting-sources).
   Build: the unversioned `source.zip` from xim.pages.dev (`1.0-SNAPSHOT`);
   the copy previously fetched here carried content dated 2026-03-09 and LSB
   tables from 2024-06-30.

## Dancer research and contributor tooling

[DancingMad](https://github.com/WGINC/DancingMad/tree/4243c7e58766691529b9612d1c71ea7bc17b4e99)
is independent research and tooling for the PC client's graphics, animation
and Dancer middleware, cross-referenced with PS2 debug data. It is not SE's
original engine source. The revision above is the one cited by the contributor's
[cow_ffxi_disassembly research vault](https://github.com/cowrevenge/cow_ffxi_disassembly),
whose README points to its Dancer ingest and DLL/DAT scanner suite.

These are research references, not build inputs under `vendor/`. They can
supply investigation leads and reproducible tooling; their claims still need
build-specific provenance and a distinction between verified findings and
inference. PS2 symbols do not alone establish current PC behavior. Apply the
same reader/writer separation and interop-record requirements as for local
binary inspection; do not transplant decompiled implementation or internal
layouts into Kuluu. Neither reference automatically overrides an applicable
retail observation or independently verified record.

## XIM

[XIM](https://xim.pages.dev/) is Aamace's from-scratch browser FFXI client
PoC (unrelated to atom0s's Xi* repos). Kept as a historical lookup aid,
not a source for vanilla acceptance criteria or parity planning.

- Live app:   <https://xim.pages.dev/>
- Source zip: <https://xim.pages.dev/source.zip>
- Docker:     <https://github.com/Masin-M/xim-docker>

**License: GPL-3.** Fetch a local copy with:

```bash
research/fetch-xim.sh
```

This downloads and extracts the source to `research/xim/`, which is gitignored
so the GPL-3 source never enters our history. Re-run the script to refresh.
