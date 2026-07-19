# Sub-target cursor (retail FFXI, observed on HorizonXI)

Observed 2026-07-18 via Parallels "Windows 11" VM, macOS-side captures (task #3).
Evidence: /tmp/subtgt-*.png series (notably -79/-80 open+zoom crop, -81-escout, -82-clear).

## Flow observed (Magic > spell on a hard-locked target)

1. Hard-lock a target (Tab/attack cursor), open Main Menu > Magic, pick a spell
   (tested: Stone).
2. Selecting the spell does NOT cast immediately. It opens the **sub-target
   cursor** to confirm/choose the recipient.

## Sub-target cursor appearance

- Small blue/purple shield-like diamond marker with a white chevron/arrow,
  **bobbing above the candidate target's head** (not at the feet).
- Candidate target's name renders in the **enlarged white outlined font** while
  the sub-target cursor is on it.
- The Magic List window (and its MP Cost panel) **hides** while the sub-target
  cursor is active.
- Top help bar shows the action prompt, e.g. `Stone — Select target of spell.`
  replacing the normal menu help line.
- Initial candidate defaults to the current hard-locked target.

## Esc behavior (unwinds one layer per press)

- Esc from sub-target: cancels the confirm step, returns toward the menu layer;
  the original **hard target lock is retained** (target frame bottom-right +
  overhead marker still present).
- Further Esc presses close remaining menus, still keeping the hard lock.
- A final Esc drops the target lock entirely -> clean idle state (no target
  frame, no marker).
- i.e. Esc never jumps straight from sub-target to "no target"; it pops one
  level of the stack per press.

## Implementation notes for local client (task #4)

- Applies to all action types that take a target: spells, job abilities,
  weaponskills, items (items quick menu already confirmed same pattern in
  task #1 observation).
- Required pieces:
  - sub-target state layered on top of (not replacing) the hard lock,
  - marker sprite above head + enlarged name styling for the candidate,
  - hide the originating list window while active, restore on cancel,
  - top-bar prompt text `"<Action> — Select target of spell."` (wording varies
    per action category),
  - Esc pops exactly one layer; confirm (Enter) fires the action at the
    sub-target and restores prior UI state.
