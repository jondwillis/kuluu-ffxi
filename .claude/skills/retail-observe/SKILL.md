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
2. hxi.sh capture         → artifacts/retail/<timestamp>.png (window-cropped)
3. hxi.sh key/type/click  → drive the game (each auto-raises the window first)
4. capture again          → before/after pairs are the useful evidence
```

**The human/agent split (durable policy):** the human does the two security
gates — clicking **Yes on UAC** and entering **account credentials** — and the
agent does everything else: window wrangling, launcher click, lobby/character
select, in-game navigation, menu driving, capture, and the code work. In
practice the launcher session persists (`Autologin activated!` in the xiloader
console), so a running session usually needs **no credential entry at all**:
`hxi.sh click-text 'Play HorizonXI'` → lobby → `Select Character` → Enter ×2
(select + "Log in with <name>?" confirm) lands in-game.

**Focus/Space snaps back the moment each shell invocation ends** — the VM
window lives on its own Space, and macOS returns to the terminal's Space
between `hxi.sh` calls. `need_window` (used by capture/click/ocr/key) now
auto-raises via the Parallels **Window → <VM name>** menu and polls up to 8s,
so every subcommand is self-contained; never assume the window stays visible
across separate invocations.

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

## OCR-verified clicking (`ocr` / `click-text`)

`hxi.sh ocr` captures + Vision-OCRs the window, printing `TEXT<TAB>x<TAB>y` in
window points. `hxi.sh click-text '<regex>'` clicks the matched text's center —
self-verifying (no blind coordinates), and it **refuses outright if an
elevation/consent (UAC) dialog is on screen** — that consent is human-only.
Prefer `click-text` over raw `click` for anything you located by reading a
screenshot; permission classifiers also accept it where a bare-coordinate
click after an unseen screenshot gets denied.

## Reading captures — the guest window drifts

The FFXI window is a movable window INSIDE the guest desktop: its position in
the capture changes between sessions (and when the user touches it). Don't
reuse pixel-crop coordinates derived from an earlier capture — they go stale
silently and you end up reading wallpaper. Either Read the full capture (large
UI text is legible at full-image scale) or re-derive crop offsets from the
current capture each time. In-game text that matters (help bar, target box,
chat) is usually readable straight off the full screenshot.

## Driving the game

- FFXI is keyboard-first; prefer keys over clicks (stable under resolution
  changes): arrows/tab for menus, `enter` confirm, `esc` cancel, `wasd`
  movement, `-`/numpad for camera. `hxi.sh key w 2.0` holds W for 2s —
  that's how you walk.
- **`hxi.sh key` accepts raw macOS keycodes as numbers** (unmapped names pass
  through): `key 44` = `/` (opens chat entry), `key 107` = Scroll Lock (retail
  toggles full-UI hide — HUD windows vanish, nameplates persist), `key 105` =
  PrintScreen. **`hxi.sh type` silently drops `/`** (and likely other
  AppleScript-special chars): to send a slash command, `key 44` first, then
  `type 'check <t>'`, then `enter`. Typing `<t>` literally works — the client
  expands it against the current target.
- After Esc closes a dialog the target is ALSO cleared — re-Tab before the
  next Enter or it re-opens nothing (a stale queued Enter can fire on the
  wrong target, e.g. the MH exit door).
- **Verify state between menu keypresses; never blind-batch.** In-game menus
  drop inputs and close unexpectedly; a queued `down/enter` sequence drifts and
  ends up interacting with the wrong entry. After each press, confirm via the
  top help bar (`hxi.sh ocr`, or crop `-c 70 1500 --cropOffset 450 1360` at 2×
  on a 1728pt window) — it shows `<Title> | <help>` for the highlighted item.
- **Tab-target check:** the Tab cycle inside a Mog House includes the exit
  Door ("Back to Town") — pressing Enter on it zones you out. Confirm the
  target bar reads the NPC you want (e.g. `Moogle`) before Enter.
- Focus is global by default: keys/clicks go to whatever window is frontmost,
  so `show` first and don't interleave with other host automation. To drive
  WITHOUT stealing focus, prefix input with `--bg` (`hxi.sh --bg key down`) —
  it posts straight to the VM process (CGEventPostToPid), sidestepping focus
  races with other apps. Experimental: confirm your Parallels build forwards
  guest input while unfocused.
- Text (chat, launcher fields): `hxi.sh type "text"` — needs focus even under `--bg`.
- **Slash commands don't land reliably**: pressing Enter opens the Commands
  menu (or targets whatever's in front), NOT a chat input, and `type '/map'`
  gets eaten by whichever menu is focused. Prefer keys + menus over chat
  commands; if a menu opened unexpectedly, Esc repeatedly before continuing.
- **Switching characters** needs no credentials while the launcher session
  persists: Play HorizonXI → Enter (agreement) → Enter (Select Character) →
  down×N — verify the char info panel (race/job/area) per press, the list
  tooltips are unreadable — → Enter → Enter ("Log in with <name>?").
- **City navigation by screenshot is slow and error-prone** (wall-hugging,
  camera collisions). Before wandering: pull exact coords from LSB
  (`vendor/server/sql/zonelines.sql`, npc_list) or the wiki, and if the user
  is around, a 20-second walk from them beats 15 minutes of capture golf.

## Delegating the drive loop (cheap models)

A drive session is mostly long runs of cheap `hxi.sh` calls — capture, ocr,
key, capture again — with little reasoning per step. Spawn a subagent with the
Agent tool using `model: haiku` to run that loop instead of spending the
orchestrating model's time and context on screenshot round-trips; keep the
judgment work (what to observe, the retail-vs-remake comparison, the report)
here.

The driver's brief must be self-contained — it inherits none of your context:

- exact commands, not intent: paste the `hxi.sh` invocations (including any
  `HXI_OWNER_RE`/`HXI_GAME_RE` overrides) rather than describing them;
- the goal phrased observably ("stop when `hxi.sh ocr` shows `Mog House`",
  not "enter the Mog House"), plus the verify-between-keypress rule above —
  a cheap model is *more* prone to blind-batching, so restate it explicitly;
- save captures under `artifacts/retail/` and return the paths plus the OCR
  lines that prove the goal state — never a bare "done";
- hard stops: UAC dialogs and credential forms are human-only — on seeing
  either, stop and report; likewise stop if two consecutive inputs change
  nothing on screen (stalled pump or drifted menu — see the gotchas above).

While a driver holds the VM window, don't run other focus-stealing automation
from this session — you'd interleave into its keystrokes. Interpretation stays
here: read the returned captures yourself before citing them for parity.

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
| Black or empty capture | Terminal lacks **Screen Recording** permission (System Settings → Privacy & Security); or the host display is asleep — hxi.sh now runs `caffeinate -u -t 2` before each capture, and for long sessions keep `caffeinate -d -u` running |
| `no visible VM window` in Coherence | Coherence windows are owned by the VM name (e.g. `Windows 11`), not `prl_vm_app`/`.exe` — owner regex now includes `^Windows 11$`; override via `HXI_OWNER_RE` if the VM is renamed |
| Session vanished between runs | Windows Update auto-restarted the VM (watch for "We've got an update for you" — dismiss with **Another time**); relaunch launcher + login afterwards |
| Keys/clicks silently ignored | Terminal lacks **Accessibility** permission; or VM window not frontmost (`show`) |
| Clicks land off-target | Pixel coords used as points — divide by the scale factor from `capture` |
| Game ignores held keys | Hold too short for a frame poll — use durations ≥0.1s (`hxi.sh key w 0.3`) |
| `prlctl exec/capture` errors | Parallels Standard — those are Pro-only; stay with hxi.sh |
| `no visible VM window` but the window IS on screen | windows_json JXA silently erroring — run its osascript without `2>/dev/null`. Historic cause: OWNER_RE spliced into the JXA unquoted, so a regex containing a space (`^Windows 11$`) word-split the program into two osascript args; fixed by passing OWNER_RE as argv. Symptom of any such bug: EVERY window query fails, which reads as "wrong Space" |
| Window on another Space right after you raised it | Focus/Space snapped back when the previous shell invocation ended — do raise+act in ONE hxi.sh call (automatic via need_window auto-raise) |
| Frontmost reads as `WinAppHelper`/`prl_vm_app` after `show` | Expected in Coherence — that shim holds focus for the guest; input still lands |
| `window` finds an `.exe`-owned window, not `prl_vm_app` | Expected in Coherence — the game window is guest-exe-owned; `hxi.sh` targets it on purpose |
| Auto-raise/`show` never raises (every query reads as wrong Space) | Some installs expose no System Events process named "Parallels Desktop" — only `prl_client_app`/`prl_vm_app`. `raise_window` now tries all of them plus a direct AXRaise on the VM console window |
| Launcher/game clicks land (OCR-verified) but have zero effect; game closes by itself | Another agent/human session is driving the same VM concurrently — inputs interleave and sessions step on each other. Do not fight for focus: stop, report the contention, and coordinate who owns the VM |
| Native Win32 popup ignores `hxi.sh click`/`key` entirely (e.g. the standalone "FINAL FANTASY XI GAMEPAD Config" tool launched from Config → Gamepad) | CGEventPostToPid input doesn't reach guest-spawned native dialogs — fall back to `osascript` `System Events` `key code …` after activating Parallels Desktop; answer its "Save changes?" prompt No unless changes were intended |

## Maintenance

When a drive session teaches something durable (launcher quirk, focus
gotcha, better key recipe), fold it into this file in the same commit.

## Opening the self Commands/Items menu (windowed VM mode)

Hard-won recipe — do NOT rediscover this:

1. If `hxi.sh show`/`window` reports no visible window, raise the VM: Parallels Desktop → Window menu → "Windows 11".
2. **Click the game sub-window's titlebar** (not the 3D viewport) to give it host keyboard focus. Without this, keys behave erratically (Enter targets a random NPC, clicks miss the HUD).
3. `key f1` — targets self. (Enter alone does NOT open a self menu; it targets the nearest NPC. Tab only cycles NPCs.)
4. `key 36` (Enter) — opens the **Commands** menu. Order: Chat, Magic, Abilities, Trust, Items, Trade, Check.
5. `key down` ×4 → Enter — opens **Items** (top bar `Items 10/20 Select an item.`; 10 items per page).
6. **Using an item takes TWO Enters**: Enter on the item opens a flashing **sub-target cursor** over the character (looks like a small blue shield/arrow above the head) — press Enter AGAIN to confirm the target and actually use the item. If you stop after the first Enter, nothing is consumed and the cursor eventually times out. Do not mistake the flashing sub-target cursor for an activation/buff indicator.

Logout: main menu (`-` key) → Log Out → confirm dialog **defaults to No** — press Left to reach Yes before Enter.

## Chat log caveats

- The colored in-game chat font is **not OCR-readable** via `hxi.sh ocr` under any observed condition.
- The chat log **fully clears/fades ~30s after the last message**. Capture item-use confirmation lines within a few seconds of the action, or skip chat and verify via the inventory list / item tooltip (stack count, recast timer) instead.

## Subagent delegation (REQUIRED)

Observation loops are screenshot-heavy and burn main-agent context. Delegate them:

- Use the Agent tool with `model: "haiku"` (haiku-4-5) for mechanical capture loops — press key / capture / read image / report what changed. Give it the exact `scripts/hxi.sh` invocations and key codes it needs.
- Use `model: "sonnet"` (sonnet-5) when the loop requires judgment (navigating unfamiliar menus, deciding next action from what's on screen, comparing against expected retail behavior).
- The main agent should only receive the subagent's *findings* (text + paths to the few decisive screenshots), never the full capture stream.
- Subagent prompts must include: window/scale info (`capture` prints it), the "divide px coords by scale for click" rule, key codes (36=Enter, up/down arrows), and hard limits on what game actions are allowed (which items/menus may be touched).
- In-game actions that consume items or charges must be explicitly listed in the subagent prompt as allowed; anything else is read-only observation.

## Autonomy

Once the mission's observations are captured, wrap up without pausing to ask "what next?": close menus and log out to the title screen as the default end state. The scope boundaries above still apply — anything that consumes items/charges or otherwise exceeds the mission's stated scope still requires explicit approval before acting.
