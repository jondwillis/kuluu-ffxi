---
name: retail-observe
description: >
  Observe and drive the real FFXI client to establish how retail behaves, on
  whatever host runs it (native Windows, Wine or a VM on macOS, Wine/Proton on
  Linux) and whatever server it talks to (a private server such as HorizonXI,
  or a local LandSandBoat stack). Capture reference screenshots, send
  keys/clicks, and compare against the Kuluu remake. Use this whenever the
  question is "how does retail / the original client do this?", whenever asked
  to capture retail reference footage or screenshots, to launch or drive the
  retail client, or to compare the remake's rendering, HUD, menus, animation,
  camera or input against the real game -- even when the request names no
  platform and never says the word "retail".
---

# Observing the retail client

Retail is the **oracle**. When a remake feature's correct behavior is unclear --
menu layout, HUD timing, animation, camera feel, spell effects -- observe it in
the real client, capture evidence, translate it into the remake, then `/verify`
the remake against the same observation.

Nothing here is tied to one machine. The client is a Windows program; the only
question is what runs it and how you reach its window. One command surface
covers every arrangement, so a recipe written on one host still reads on the
others.

## Recorded observations -- read before driving anything

`references/` is this repository's durable record of how retail behaves. A
login costs minutes and a drive session costs far more, so check whether the
question is already answered before spending either.

- [Vanilla menu & target-interaction spec](references/vanilla-menu-spec.md) --
  target-action menu, trade window, item detail, `/check`, Commands, Status,
  Items + sort. The broadest single record.
- [Items window](references/2026-09-11-items-window.md) -- the main-menu Items
  list: layout, 10-row paging and scroll rules, Options box, Item submenu
  (Use/Drop), verbatim help-bar strings.
- [Auction House](references/auction-house.md) -- category tree, screens, sell
  flow, Sales Status, bid/browse, and how catalog paging is pull-based.
- [NPC shop window](references/2026-09-15-shop-window.md) -- why a shop is not an
  event and has no close packet, the client's 80-entry zone-scoped stock table,
  the four `shop*` menu primitives, and the sell handshake. Binary/packet-doc
  observation; the on-screen layout is still unpinned.
- [Mog House menu](references/2026-07-17-moghouse-menu.md) -- exact entry order
  for the main menu, Storage, Delivery Box send/receive, Change Jobs.
- [Compass radar](references/2026-09-09-compass-radar.md) -- establishes the
  standing compass as vanilla HUD; separates the terrain minimap question,
  which it does *not* establish.
- [Death / KO behavior](references/death-ko-behavior.md) -- collapse motion,
  corpse hold, homepoint timer wire facts, a lifecycle gotcha.
- [Sub-target cursor](references/sub-target-cursor.md) -- appearance, when it
  opens, and how Esc unwinds one layer per press.
- [Treasure pool chat lines](references/treasure-pool-chat.md) -- wording and
  colour selection, and which parts are still unpinned.
- [Hatchling Shield](references/hatchling-shield.md) -- Items menu flow, exact
  tooltip text, use behavior, plus observation-method gotchas worth reading
  before any item-use run.
- [NPC animation routine selector](references/2026-09-08-npc-animation-selector.md)
  -- binary observation rather than a live drive; the computation and its
  server cross-check.

Record new durable findings here as dated observation records, per the routing
rule in the root `AGENTS.md`. Captures and binary dumps stay local.

## One command surface, any host

`scripts/observe.sh` (macOS, Linux) and `scripts/observe.ps1` (native Windows)
implement the same verbs against different host mechanics. Run either with no
arguments for the full table; the shape is:

```
doctor         prerequisites, permissions, and whether a client window resolves
targets        every window this host reports -- find yours here, don't guess
status/window  which window resolved as the client, and its geometry
show           raise/focus it
capture [out]  screenshot the client window, cropped, with its scale factor
ocr            capture + OCR: TEXT<TAB>x<TAB>y in window units
keys           the logical key names and what each does in game
key <name> [s] press or hold a key, by logical name
type <text>    type a string
click/move/drag/click-text   pointer input; click-text is OCR-verified
launch         start the client from a profile, optionally against a server
```

**Observing a client that is already running needs no configuration.** The
client titles its window `FINAL FANTASY XI` on every host, which is the default
match. Everything else is an env override (`FFXI_OBSERVE_WINDOW_TITLE`,
`FFXI_OBSERVE_VM_NAME`, ...) or a named profile, and only `launch` needs one.

Start every session with `doctor`: it reports missing tools, ungranted host
permissions, and whether the client is up, which is faster than interpreting a
black screenshot. When a window will not resolve, `targets` shows what the host
actually sees -- a launcher window, a config tool, a VM console -- so you fix
the title regex instead of guessing.

Host mechanics, permissions and failure modes live in one reference each. Read
the one for your host before debugging the tool:

| Host | How the client runs | Reference | Status |
|---|---|---|---|
| macOS | Wine (FFXI-on-Mac, Whisky, CrossOver, plain wine) or a VM (Parallels, VMware, UTM) | [host-macos.md](references/host-macos.md) | verified |
| Windows | natively | [host-windows.md](references/host-windows.md) | written, unverified |
| Linux | Wine, Proton (umu-run), Steam Deck | [host-linux.md](references/host-linux.md) | written, unverified |

"Unverified" means the mechanics come from documented platform behavior rather
than a run against a live client. Treat a failure there as a bug to fix in the
backend, not as a reason to hand-roll a parallel script -- `doctor` is designed
to localize exactly that.

Getting from an install on disk to a character in a zone -- loaders, pointing
the client at a private server or at a local LandSandBoat stack, resolution and
config traps, who types credentials -- is
[client-and-server.md](references/client-and-server.md).

## Session flow

```
1. observe.sh doctor            host ready? client up?
2. observe.sh capture           artifacts/retail/<timestamp>.png
3. observe.sh key/type/click    drive the game
4. capture again                before/after pairs are the useful evidence
```

**Each invocation is self-contained, and must be.** Focus does not survive
between shell calls on any host: the window manager hands focus back to the
terminal the moment a command exits. Every input verb therefore re-resolves and
re-raises the client window inside the same invocation. Do not "optimize" that
away by raising once and firing a batch of keys -- the keystrokes land in
whatever is frontmost, usually the terminal you are driving from.

## The human/agent split

The human does two things: **answer consent prompts** (UAC, a macOS permission
sheet, polkit) and **enter account credentials for a server they do not own**.
The agent does everything else -- window wrangling, launcher, lobby, character
select, navigation, menu driving, capture, and the code work.

`click-text` enforces the first half: it refuses outright while a consent
dialog is on screen, on every host, so no drive loop can talk itself into
clicking one. For the second, credentials for someone else's server are theirs;
hand off rather than typing stored secrets. A **local** LSB stack is different
-- those are throwaway test accounts you own, and driving them end to end is
fine (see client-and-server.md).

## Driving the game

Keys are named for what they do in game, not for a host keycode:
`observe.sh keys` prints the table, and `lib/keys.tsv` is the one place a key is
defined for all three hosts. Write recipes with logical names (`key zoom-out`)
so they survive being read on another platform; a raw keycode in a recipe is a
recipe that only works where it was written.

- FFXI is keyboard-first; prefer keys over clicks -- they are stable across
  resolution changes. Arrows/`tab` for menus, `enter` confirm, `esc` cancel,
  `w`/`a`/`s`/`d` movement. `key w 2.0` holds W for two seconds; that is how
  you walk. A hold shorter than the client's input poll is invisible to it, so
  use durations of at least 0.1s.
- **`zoom-out` / `zoom-in` are the chase camera**, and each runs to a stop. To
  confirm you are AT the stop, hold again and diff the two captures -- a
  partial hold looks exactly like a clamp. Chase cam itself is a Config toggle,
  not a key: Commands -> Config -> Mouse/Camera -> `Camera View: Chase Cam`.
- **`cam-up` / `cam-down` change camera HEIGHT, not orbit** -- retail adds to
  the eye's world Y and leaves the horizontal offset alone, so the character
  shrinks as you raise it. Also runs to a stop.
- **Neither mouse drag moves the camera** in the observed config: left and
  right drag across the full viewport both changed nothing. Use the keys; do
  not conclude `drag` is broken.
- **`menu` opens the main menu and Log Out is on page TWO** -- left/right switch
  pages (the arrows flanking the highlighted entry), Log Out is the 12th entry
  on page 2, and its confirm dialog defaults to **No**, so press `left` before
  `enter`. Logout then runs a ~20s countdown; kneeling or healing does not
  block it.
- **`hud-toggle` hides every HUD window** (nameplates persist) -- do that before
  a geometry or collision capture, or chat and menus sit on top of exactly what
  you are comparing.
- **Verify state between menu keypresses; never blind-batch.** In-game menus
  drop inputs and close unexpectedly, so a queued `down/enter` sequence drifts
  and ends up acting on the wrong entry. After each press, confirm via the top
  help bar (`observe.sh ocr`), which reads `<Title> | <help>` for the
  highlighted item.
- **After Esc closes a dialog the target is also cleared** -- re-`tab` before the
  next `enter`, or a stale queued Enter fires on the wrong target.
- **Tab-target trap:** the Tab cycle inside a Mog House includes the exit door
  ("Back to Town"), and Enter on it zones you out. Confirm the target bar reads
  the NPC you want before pressing Enter.
- **Slash commands are unreliable** -- Enter opens the Commands menu or targets
  whatever is in front rather than opening a chat input. Prefer keys and menus.
  Where a slash command is unavoidable, press `chat` first, then `type` the
  command body, then `enter`; typing `<t>` literally works, since the client
  expands it against the current target. On macOS `type` drops a literal `/`
  (AppleScript eats it), which is exactly why `chat` exists as a key.
- **Switching characters** needs no credentials while the launcher session
  persists: Play -> Enter (agreement) -> Enter (Select Character) -> `down` x N,
  verifying the character info panel (race/job/area) on each press because the
  list tooltips are unreadable -> Enter -> Enter.
- **City navigation by screenshot is slow and error-prone** (wall-hugging,
  camera collisions). Before wandering, pull exact coordinates from LSB
  (`vendor/server/data/zones/<zone>/zone.yaml` zonelines, npc_list) or the
  wiki. If the user is around, a 20-second walk from them beats 15 minutes of
  capture golf.

### Opening the self Commands/Items menu

Hard-won; do not rediscover it:

1. `key target-self` -- targets self. Enter alone does not open a self menu, it
   targets the nearest NPC, and Tab only cycles NPCs.
2. `key enter` -- opens **Commands**: Chat, Magic, Abilities, Trust, Items,
   Trade, Check.
3. `key down` x4 then `enter` -- opens **Items** (help bar reads
   `Items 10/20 Select an item.`; 10 items per page).
4. **Using an item takes TWO Enters.** The first opens a flashing sub-target
   cursor over the character; the second confirms the target and actually uses
   it. Stopping after the first consumes nothing and the cursor eventually times
   out. Do not mistake that cursor for an activation or buff indicator.

### Chat log caveats

- The coloured in-game chat font is **not OCR-readable** under any observed
  condition, on any host.
- The chat log **clears ~30s after the last message**. Capture a confirmation
  line within a few seconds of the action, or skip chat entirely and verify via
  the inventory list or item tooltip (stack count, recast timer).

## Coordinates -- the one real trap

`click`/`move`/`drag` take coordinates relative to the client window's
top-left, in the host's logical units. Captures are in device **pixels**, which
on a HiDPI display is a larger number -- `capture` prints the exact scale
factor, and any coordinate you measured in a screenshot must be divided by it
before clicking. Re-resolve the window after any move or resize; coordinates do
not survive it.

Prefer `click-text '<regex>'` over raw coordinates for anything you located by
reading a screenshot. It OCRs, clicks the matched text's centre, and fails loudly
when the text is not there -- self-verifying, where a bare coordinate click
silently lands on wallpaper. Permission classifiers also tend to accept it where
they deny a blind coordinate click.

## Reading captures

The client window moves. Its position inside a capture changes between sessions
and whenever the user touches it, so pixel-crop offsets derived from an earlier
capture go stale silently and you end up reading wallpaper. Either read the full
capture -- large UI text is legible at full-image scale -- or re-derive crop
offsets from the current capture each time. In-game text that matters (help bar,
target box, chat) is usually readable straight off the full screenshot.

## Delegating the drive loop

A drive session is mostly long runs of cheap calls -- capture, ocr, key, capture
again -- with little reasoning per step. Spawn a subagent on a cheap model to
run that loop instead of spending the orchestrating model's context on
screenshot round-trips, and keep the judgment here: what to observe, the
retail-vs-remake comparison, the report. Use a capable model instead when the
loop itself needs judgment (navigating unfamiliar menus, deciding the next
action from what is on screen).

The driver's brief must be self-contained -- it inherits none of your context:

- exact commands, not intent: paste the invocations, including any
  `FFXI_OBSERVE_*` overrides, rather than describing them;
- the goal phrased observably ("stop when `ocr` shows `Mog House`", not "enter
  the Mog House"), plus the verify-between-keypresses rule -- a cheap model is
  *more* prone to blind-batching, so restate it;
- window and scale facts (`capture` prints them) and the divide-px-by-scale
  rule;
- save captures under `artifacts/retail/` and return the paths plus the OCR
  lines that prove the goal state -- never a bare "done";
- hard stops: consent dialogs and credential forms are human-only; stop and
  report. Also stop if two consecutive inputs change nothing on screen (a
  stalled client or a drifted menu).
- an explicit allow-list for anything that consumes items or charges.
  Everything not listed is read-only observation.

While a driver holds the client window, do not run other focus-stealing
automation from this session -- you would interleave into its keystrokes.
Interpretation stays with you: read the returned captures yourself before citing
them for parity.

## Capturing evidence for parity work

- Save to `artifacts/retail/` (the capture default). Name pairs explicitly when
  comparing: `retail-moghouse-menu.png` against the remake's screenshot from
  `/verify`'s GUI surface.
- Retail DATs and captures of them are SE-copyrighted: reference material only,
  never committed. Installs live outside the repository -- find one with
  `kuluu install path NAME`. Keep captures local and quote paths, not pixels, in
  reports and beads.
- For animated behavior, capture a burst:
  `for i in 1 2 3 4 5; do scripts/observe.sh capture; sleep 0.5; done`.

## Autonomy

Once the mission's observations are captured, wrap up without pausing to ask
"what next?": close menus and log out to the title screen as the default end
state. Anything that consumes items or charges, or otherwise exceeds the
mission's stated scope, still needs explicit approval first.

## Maintenance

When a drive session teaches something durable, fold it into the same commit:
a game fact into this file, a host quirk into that host's reference, a new key
into `lib/keys.tsv` so every host gets it at once. A recipe that only works on
the machine it was written on is the thing this layout exists to prevent.
