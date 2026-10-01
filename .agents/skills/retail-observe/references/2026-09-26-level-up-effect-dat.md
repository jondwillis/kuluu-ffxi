# Level-up effect DAT, 2026-09-26

Static mapping of the level-up effect routine and its file id in this install
(`C:\PhoenixXI\SquareEnix\FINAL FANTASY XI`, VTABLE/FTABLE walk). Kuluu plays
the SFX half of s2c `0x029` BATTLE_MESSAGE msg_num=9 already (kuluu-session
`emit_battle_message_audio_event` → `AgentEvent::LevelUp`); this record pins the
motion half: which file, and under what routine name.

## Findings

- The level-up effect lives in **ROM/13/35.DAT**, file id **3310** on this
  install's tables (only the base ROM claims it; no expansion table shadows the
  id). Its type-0x01 marker chunk is named `lvup`, but its single routine
  (type-0x07) is named **`main`** — the effect-DAT pattern kuluu already runs for
  spell effects, not a named-routine lookup.
- The sibling level-down effect lives in ROM/13/34.DAT, file id **3309**, marker
  `lvdw`. No kuluu trigger maps to it (msg_num=53 is skill-up, a different table).
- `main`'s stages: VFX generators (`g0s0`, `g000`..`g004`) plus a linked call to
  `mdam` at frame 40 — the same damage-callback routine spell completions link,
  so it resolves through the file → actor → global-dir tiers like any other
  effect DAT. Skeleton clips `lvu1`..`lvu4` ship in the file itself (type-0x20/0x21).

## Reproduce

```
python ffxi_disassembly/ffxi_dat_find.py resolve 3310 3309
python ffxi_disassembly/dat_routines.py <install>/ROM/13/35.DAT
```

The first prints `3310 ROM\13\35.DAT` / `3309 ROM\13\34.DAT`; the second dumps
the marker, generators, and the 14-stage `main` routine. File ids are per-install
table state: re-run the resolve before trusting a const on another install.
