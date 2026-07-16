---
name: retail-observe
description: >
  Observe and drive the retail FFXI client (HorizonXI) running in the
  Parallels "Windows 11" VM on this macOS host — capture reference
  screenshots of retail behavior, send keys/clicks to the game, and compare
  against the Kuluu remake for feature parity. Use whenever asked to check
  how retail/the original client does something, capture retail reference
  footage/screenshots, drive the retail client, or compare remake rendering,
  HUD, menus, animations, or behavior with the real game.
---

# Observing the retail client (HorizonXI in Parallels)

Purpose: retail is the **oracle**. When a remake feature's correct behavior
is unclear — menu layout, HUD timing, animation, camera feel, spell effects —
observe it in the real client, capture evidence, translate into the remake,
then `/verify` the remake side against the same observation.

Everything runs host-side through `scripts/hxi.sh` (this machine has
Parallels **Standard**: no `prlctl exec`/`prlctl capture`; the VM is reached
via the macOS window server instead). Run `hxi.sh` with no args for the
command table.

## Session flow

```
1. hxi.sh status          → VM running? window visible?
2. hxi.sh show            → focus + raise the VM window (input lands only when focused)
3. hxi.sh capture         → artifacts/retail/<timestamp>.png (window-cropped)
4. hxi.sh key/type/click  → drive the game
5. capture again          → before/after pairs are the useful evidence
```

- VM not running: `prlctl start "Windows 11"`, then wait for the launcher.
  Getting from Windows desktop → in-game goes through HorizonXI-Launcher, then
  the FFXI loader. From the host you can launch either via its Parallels app
  stub: `open ~/"Applications (Parallels)"/*Applications.localized/"HorizonXI-Launcher.app"`
  (the game window itself is `horizon-loader.exe.app` in the same folder).
- **Resolution gotcha:** if the loader won't open a game window, the FFXI
  background resolution is likely too high — 4096² wedged it; **2048² works**.
  Set it in the launcher/FFXI Config before blaming `hxi.sh`.
- **Launcher quirk:** the HorizonXI-Launcher window is owned by
  `HorizonXI-Launcher` (not matched by the default `OWNER_RE`) — drive it with
  `HXI_OWNER_RE='HorizonXI-Launcher|...' HXI_GAME_RE='HorizonXI Launcher'`. Its
  launch **ticket expires within seconds**: click the green "Ready! Click to
  Launch" *fast* (or fire `--bg` clicks in a tight loop). A crashed prior
  session shows "Ticket not found, expired, or already used" until the server
  drops the ghost session (~minutes) — wait it out; don't Logout (re-login
  needs the account password).
- **Slow-VM pump stalls:** on a slow disk the client's UI thread stalls and the
  title shows "(Not Responding)" during loads; input sent then is *dropped*.
  The 3D background keeps animating (separate thread), so a live capture ≠ a
  responsive pump. Send confirm keys only once the title clears, then be patient.
- Window visible but wrong Space: `hxi.sh show` raises it; if that fails the
  window is closed — reopen from Parallels Desktop (Window → Windows 11).
- **Ask the user before logging in.** Retail credentials are theirs; if the
  launcher sits at a login form, hand off rather than typing stored secrets.

## Coherence mode — why the window looks like a macOS app

The game usually runs in Parallels **Coherence** (guest windows blended onto
the macOS desktop). That inverts a few intuitions `hxi.sh` already accounts
for — worth knowing when debugging:

- The game window's CGWindow **owner is the guest exe** (`horizon-loader.exe`),
  *not* `prl_vm_app`. `hxi.sh`'s `OWNER_RE` matches `[.]exe$` and `vm_window`
  prefers the window whose title matches `GAME_RE` ("FINAL FANTASY"), so it
  targets the game directly rather than the VM console.
- The title carries **trailing NUL bytes** (`FINAL FANTASY XI\0\0`).
  Matching is substring, so this is harmless — don't match on exact equality.
- `screencapture -l <id>` grabs the window **even across Spaces / when macOS
  reports it not on-screen** — so `hxi.sh capture` works without fighting the
  window into the foreground. `hxi.sh show` (needed only for *input* focus)
  raises `prl_vm_app`; after that the **frontmost process reads as
  `WinAppHelper`** (the Parallels input shim). That is success, not a failure —
  keys/clicks still land in the guest.
- Classic (non-Coherence, full-screen VM) windows go through `prl_vm_app`
  normally; `hxi.sh` falls back to that path automatically.

Observed cold-start sequence (so a run knows what to expect): PlayOnline user
agreement (**Accept**) → "Acquiring FINAL FANTASY XI server data" → HORIZON XI
character-select (Select / Create / Delete / Config / Back). Character-select
is the login handoff point.

## Coordinates — the one real trap

`hxi.sh click/move` take points **relative to the VM window's top-left, in
macOS points**. Screenshots are Retina **pixels** (~2× points). `capture`
prints the exact scale factor; divide pixel coordinates you measured in a
screenshot by it before clicking. Re-run `hxi.sh window` after any window
move/resize — coordinates don't survive it.

## Driving the game

- FFXI is keyboard-first; prefer keys over clicks (stable under resolution
  changes): arrows/tab for menus, `enter` confirm, `esc` cancel, `wasd`
  movement, `-`/numpad for camera. `hxi.sh key w 2.0` holds W for 2s —
  that's how you walk.
- Focus is global by default: keys/clicks go to whatever window is frontmost,
  so `show` first and don't interleave with other host automation. To drive
  WITHOUT stealing focus, prefix input with `--bg` (`hxi.sh --bg key down`) —
  it posts straight to the VM process (CGEventPostToPid), sidestepping focus
  races with other apps. Experimental: confirm your Parallels build forwards
  guest input while unfocused.
- Text (chat, launcher fields): `hxi.sh type "text"` — needs focus even under `--bg`.

## Capturing evidence for parity work

- Save to `artifacts/retail/` (already the capture default). Name pairs
  explicitly when comparing: `retail-moghouse-menu.png` vs the remake's
  screenshot from `/verify`'s GUI surface.
- Retail DATs and captures of them are SE-copyrighted — reference material
  only, never committed (see `.gitignore` on `vendor/game-files/`). Keep
  captures local; quote paths, not pixels, in reports/beads.
- For animated behavior, capture a burst: `for i in 1 2 3 4 5; do
  scripts/hxi.sh capture; sleep 0.5; done`.

## Troubleshooting

| Symptom | Cause → fix |
|---|---|
| `no visible VM window` | VM headless/minimized/other Space → `hxi.sh show`, else reopen in Parallels Desktop |
| Black or empty capture | Terminal lacks **Screen Recording** permission (System Settings → Privacy & Security) |
| Keys/clicks silently ignored | Terminal lacks **Accessibility** permission; or VM window not frontmost (`show`) |
| Clicks land off-target | Pixel coords used as points — divide by the scale factor from `capture` |
| Game ignores held keys | Hold too short for a frame poll — use durations ≥0.1s (`hxi.sh key w 0.3`) |
| `prlctl exec/capture` errors | Parallels Standard — those are Pro-only; stay with hxi.sh |
| Frontmost reads as `WinAppHelper`/`prl_vm_app` after `show` | Expected in Coherence — that shim holds focus for the guest; input still lands |
| `window` finds an `.exe`-owned window, not `prl_vm_app` | Expected in Coherence — the game window is guest-exe-owned; `hxi.sh` targets it on purpose |

## Maintenance

When a drive session teaches something durable (launcher quirk, focus
gotcha, better key recipe), fold it into this file in the same commit.
