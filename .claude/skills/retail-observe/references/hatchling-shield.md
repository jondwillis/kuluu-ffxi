# Hatchling Shield — retail observations (HorizonXI, retail FFXI client)

Recovered from observer-agent transcript `a4cbbcb42715a5c93` (session
`d78499d2-e0fe-4138-84b0-8cc9f510ee5d`, 2026-07-18). Screenshots were local-only
(`/tmp/hxi3-hatchling.png`, `/tmp/hxi3-use-05s.png`, `/tmp/hxi3-use-40s.png`) and
may no longer exist.

## Items menu flow (retail)

1. `F1` — target self (required; Enter alone targets nearest NPC).
2. `Enter` — opens **Commands** menu, cursor on Chat.
   Menu order: Chat, Magic, Abilities, Trust, **Items**, Trade, Check.
3. Down ×4, Enter — opens **Items** (top bar: `Items 10/20 Select an item.` —
   10 items per page of 20 total), cursor starts on first stack.

## Hatchling Shield tooltip (exact)

```
Hatchling shield
All Races
DEF: 1
Dispense: Eggs
Lv.1 All Jobs
<1/1 0:00/[24:00:00, 0:30]>
```

- Charges 1/1, recast shown `0:00` when ready, reuse `24:00:00`,
  activation window `0:30`.

## Use behavior (retail)

- On use: small blue shield icon appears above the character's head immediately
  and persists through the activation window (still visible at t=+40s).
- The Items submenu auto-closes to a bare `Items` top bar after use.
- Chat confirmation line was NOT captured (colored in-game chat font is not
  OCR-readable; chat log fully clears/fades within ~30s of no new messages).
- Bird Egg tooltip: `Bird egg / This egg is renowned for its flavor. HP+6 MP+6 /
  Duration: 5 minutes.` Use decrements stack (8→7); chat shows
  `Oldman uses a bird egg.` — food buffs apply silently (no "gains the effect"
  line), unlike spell buffs.

## Local client verification (task #2 outcome)

Verified in-session against the local client: dispense charge consumed and egg
delivered to inventory; menu flow, tooltip charge/recast format, and silent
food-buff application match retail. Sub-target cursor flash on self during
item activation is covered in `sub-target-cursor.md`.

## Observation-method gotchas (for future runs)

- Click the game sub-window's **titlebar** first to give it host keyboard focus
  (windowed/non-Coherence Parallels mode) — input is erratic otherwise.
- Capture chat confirmation within a few seconds of pressing Enter, or verify
  via inventory counts instead of chat.
- Tab only cycles nearby NPCs; it never opens self menus.
