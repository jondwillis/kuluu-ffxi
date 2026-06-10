# Kuluu

A Rust + Bevy client for the FINAL FANTASY XI protocol. Native window, chase
camera, retail-feel HUD, and a parity scoreboard tracked honestly against the
official client.

Kuluu is a **game-preservation effort**. FINAL FANTASY XI is a Square Enix
property; this repo has no affiliation with, and no endorsement from, Square
Enix. If you enjoy FFXI, please support the official service.

## Setup / first build

The workspace builds from a clean checkout once the few **build-time** vendor
submodules are present. Build scripts read data files out of them and translate
the values into compile-time Rust constants; no copyrighted asset bytes leave
the user's machine, and the submodules are *not* needed at runtime.

```bash
git clone <repo> ffxi && cd ffxi

# Init only the submodules the build actually reads (shallow — history trimmed).
# `server` is large; --depth 1 keeps the working tree without the full history.
git submodule update --init --depth 1 \
  vendor/server vendor/POLUtils vendor/AltanaListener

# ffxi-navmesh-builder and recastnavigation-rs are vendored in-tree (no submodule).
cargo build
```

That's everything the compiler needs. Upstream repos that are **not used by
the build** — only cited in source comments for reference (`Phoenix`,
`AltanaViewer`, `lotus-ffxi`, `xi-tinkerer`) — live under `research/`, not
`vendor/`. They stay deinitialized; `git submodule update --init research/<name>`
populates one if you want to read the upstream sources.

To actually *run* the client you also need a user-provided retail install
(~19G, never committed — see [Getting the game files](#getting-the-game-files)).

> **Shallow-clone caveat:** `--depth 1` works only while the pinned submodule
> commit is still reachable from its tracked branch tip. If an upstream
> force-push moves it out of reach, re-run without `--depth` (or with a larger
> `--depth N`) for that submodule.

## Getting the game files

The FFXI client DATs (geometry, textures, audio, animations) are Square Enix
copyrighted and must come from a **legitimate install** — Kuluu never ships or
commits them. The client reads them from `vendor/game-files/` by default
(gitignored), or wherever `FFXI_DAT_PATH` points.

Get an install one of these ways:

- **HorizonXI launcher (Windows):** install via <https://horizonxi.com>; its
  launcher downloads a full FFXI + Ashita tree.
- **Lutris (Linux):** <https://lutris.net/games/horizonxi/>. Files land under
  `~/Games/.../drive_c/.../SquareEnix/FINAL FANTASY XI/`.
- **Copy an existing install:** the PlayOnline tree from any retail/private-server
  install (`.../PlayOnline/SquareEnix/FINAL FANTASY XI/`).

Kuluu expects this layout (the parent of `SquareEnix/`):

```
vendor/game-files/
  SquareEnix/
    FINAL FANTASY XI/      <- FFXI_DAT_PATH points here
      VTABLE.DAT  FTABLE.DAT
      ROM/  ROM2/ … ROM9/
      sound/win/…
```

Then wire it up with the cross-platform helper, which detects an existing
install (HorizonXI / Lutris / Wine / CrossOver / PlayOnline), validates it, and
symlinks it into `vendor/game-files/`:

```bash
cargo xtask game                 # auto-detect
cargo xtask game "/path/to/..."  # or point it at a known install
cargo xtask game --copy          # copy instead of symlink
```

Don't have an install yet? The helper can also download Square Enix's **official**
client installer from the public PlayOnline CDN and launch it (opt-in and
confirmation-gated; runs under Wine on macOS/Linux). The download is free; a
registration code / subscription is needed to play on the official service:

```bash
cargo xtask game --download             # official US client; prompts first
cargo xtask game --download --region eu
```

Complete the installer GUI, then run `cargo xtask game` to wire it up. (This is
official-client only — HorizonXI and other flavors must be obtained through
their own launchers.)

Or do it by hand — drop/symlink your install at `vendor/game-files/`, or just
point the client at an existing copy:

```bash
export FFXI_DAT_PATH="/path/to/.../SquareEnix/FINAL FANTASY XI"
```

`FFXI_DAT_PATH` also overrides at runtime and can be set from the launcher's
settings UI, so you never have to move a large install to use it.

## Project goals & non-goals

- **Vanilla parity is the base.** The aim is close to 1:1 with the official
  FFXI client in default mode — same menus, same compass, same combat feel.
  Anything that has no retail equivalent lives in the **Enhanced / addon**
  scoreboard below, not the Vanilla one.
- **Modernization layers on top, opt-in.** Bevy + wgpu replace the legacy
  D3D8/D3D11 stack; a planned plugin/extension API aims to obviate
  Windower/Ashita. Enhanced features (WoW-style minimap, DPS meter, gear
  swap, …) ship as opt-in flavors/builds/packs that compose with vanilla,
  never replace it.
- **No asset redistribution.** Kuluu requires a user-provided retail install
  (e.g. mounted at `vendor/game-files`). Translated tables vendored from LSB,
  POLUtils, AltanaListener, etc. are stored as derived constants under the
  upstream license — never as game content.
- **Not for retail.** The retail FFXI service enforces anti-cheat and Terms
  of Service that Kuluu makes no attempt to honor. Do not point it there.
- **Not a server.** Kuluu speaks to community-run FFXI-protocol servers
  (LSB / Phoenix). Each such server has its own rules — read them before
  you log in.

## Crates

The workspace is split so the protocol, parsers, and renderer can evolve
independently and so the agent/MCP harnesses can reuse the client without
pulling in Bevy.

- `ffxi-proto` — wire protocol for LSB / Phoenix private servers.
- `ffxi-client` — interactive client; the `play` subcommand opens the GUI
  window by default, or `play --headless` runs the JSON event-stream agent
  session. Login, session, input, reactor.
- `ffxi-viewer-core` — Bevy systems: scene graph, HUD, audio, minimap,
  camera, sky/weather, picking.
- `ffxi-viewer-wire`, `ffxi-viewer-wasm` — viewer transport + browser build.
- `ffxi-dat` — native parsers for retail DAT files (VTABLE/FTABLE, MZB,
  MMB, ANI, textures).
- `ffxi-audio` — BGW/SPW container parsing + ADPCM/PCM decode (ported from
  lotus-ffxi).
- `ffxi-nav` — in-house 2D/3D pathfinding for agent harnesses.
- `ffxi-nav-recast` — Recast/Detour navmesh sourced from
  LandSandBoat/xiNavmeshes.
- `ffxi-mcp` — MCP server bridging `ffxi-client` to LLM harnesses (Claude
  Code, OpenCode, pi.dev).
- `ffxi-agent` — agent-side glue around the MCP bridge.

## Run

Credentials come from environment variables so they never end up in logs or
shell history:

```bash
export FFXI_USER=...
export FFXI_PASS=...
export FFXI_CHAR=...           # exact display name
export FFXI_SERVER=127.0.0.1   # server hostname or IP
```

Native window (the default `play` mode; GUI ships by default):

```bash
cargo run -p ffxi-client -- play
```

Headless (JSON-line agent session, useful for protocol work):

```bash
cargo run -p ffxi-client --no-default-features -- play --headless
```

If any credential env var is unset, the launcher prompts for it and lists
characters on the account so you can pick by name.

## Retail-parity scoreboard

Two scoreboards: **Vanilla parity** tracks 1:1 retail-client features.
**Enhanced / addon** tracks opt-in modernization layers with no retail
analog. A feature only ever lives on one of the two lists.

Legend: `[x]` implemented · `[~]` partial (decoded or scaffolded, UI or
dispatch incomplete) · `[ ]` missing · `[?]` not investigated.

`TODO(litigate)` marks lines where the author and contributors have not yet
agreed on scope or accuracy — flip to a concrete state in the PR that
resolves it.

### Vanilla parity

#### World & rendering

- [~] Zone geometry — entity sync + FFXI→Bevy coord transform work; many
  zones still render with holes or missing interiors.
  - [ ] Bastok Markets: missing floors in places
  - [ ] Southern San d'Oria: missing interior houses
  - [ ] `TODO(litigate)` enumerate other broken zones — non-exhaustive,
    contributions welcome
- [x] PC + NPC skinned meshes
- [ ] Airships, boats, elevators
- [ ] Interactible doors (open/close, zone transition surfaces) — incl. the
  retail door flow: yes/no confirm dialog → open animation → fade-to-black →
  "Downloading data" indicator → fade-in → zone changed
- [x] Skybox dome with sRGB-correct lerp
- [~] Sun + moon — billboards render; arc, color, and Vana-day timing not
  verified against retail
- [x] Weather visual FX
- [x] Music playback, per-channel mute
- [~] Sound effects — scheduler runs; action→sound table not yet mapped
- [x] Picking, hover-cursor, entity hover card
- [x] Chase camera + collision

#### HUD

- [ ] Retail compass (minimal entity-direction widget shown in the official
  client). Note: the existing minimap implementation is an addon — see
  Enhanced below.
- [~] Nameplate billboards — distance-to-scale curve still placeholder
- [x] Stage bar, chat panes (Social / Battle / Debug), self HUD, party
  roster, target panel, Vana clock, weather icon, zone flash, dialog
  window, death prompt, logout countdown, status ribbon
- [ ] Self HUD solo / party indicator (shown in the bottom-right self panel
  alongside name + HP/MP bars)
- [ ] Target panel width parity — target panel must match the self/party
  panel width (currently 280px vs 220px)
- [ ] **Target-action contextual menu** (vanilla): select an entity → confirm
  opens Chat / Magic / Abilities / Trust / Items / Trade / Check. Chat is a
  cycle-button (Say→Tell→Party→Linkshell→Unity→Shout, default `/tell`); each
  entry has contextual visibility and "no X available" / "cannot use that here"
  error strings. This is the vanilla spine; **not** the Enhanced quick-action
  ring (see Enhanced below).
- [ ] Item detail / tooltip panel (docked bottom-left, replaces compass+clock
  while open): icon, name, rare/ex, slot, race/job, level, enchantment, uses
  remaining + recast, and consumable status-effect grant + duration
- [~] Main menu (`-`), title "Commands": Equipment, Magic, Abilities, Items
  submenus wired; Status, Party, Search, Macros, Graphics submenus still
  stubbed. Order/contents diverge from retail's two-column layout (Status,
  Equipment, Magic, Items, Synthesis, Abilities, Party, Trade, Search,
  Linkshell, Region Info, Map | Missions, Quests, Key Items, View House,
  Macros, Config, Help Desk, Time, Communication, Shut Down, Log Out)
- [ ] Full-screen region map, quest markers, zone-line glyphs
- [ ] Recast-timer display
- [ ] Buff / debuff duration timers
- [ ] Macro palette editor

#### Combat & action

- [x] Target lock + tab-cycle (by distance)
- [x] Auto-attack engage / disengage, engaged-target ring
- [x] Combat-stance animation
- [x] `/check` — mob con (level / defense estimate)
- [ ] `/check` on players — bazaar "View Wares" + 4×4 equipment grid
  inspector, with per-slot item detail and a "`<name>` examines you" chat line
  (confirmed: this is the "item /check" from the old scoreboard)
- [~] Action dispatch from menu — item use wired; magic / ability dispatch
  partial
- [ ] Magic list spell categories (White / Black / Songs / Summoning / Blue;
  classic set) with a "No spells available" empty state per category
- [ ] Abilities sub-grouping: Job Abilities / Weapon Skill / Ranged Attack /
  Mount / Pet Command, with contextual error strings ("You cannot use that
  command here.", "No mounts available.")
- [ ] Weaponskill chain UI (vanilla skillchain message + animation, not the
  addon visualizer)

#### Inventory & equipment

- [~] Main inventory: decoded and browsable; sub-containers (Sack, Satchel,
  Case, Wardrobe, Mog Locker, Gobbiebag) data-ready, no per-container view
- [~] Equipment: 16 slots displayed, equip-from-inventory works — unequip
  and basic gear management missing
- [ ] Stack split, drop, sort — sort menu has Auto (yes/no → server sort),
  Manual (swap positions), Recycle Bin (recently discarded); Manual + Recycle
  Bin are incomplete even upstream, so Auto is the parity target. Item list
  shows held/capacity count (e.g. 14/30) and a "Select an item." helper

#### Party & social

- [x] Party state, HP / MP %, low-HP event
- [x] Chat send / receive on Say, Shout, Tell, Party, Linkshell, Yell,
  System
- [ ] Linkshell 2 + linkshell management UI
- [ ] `/search` + player search panel
- [ ] Search-comment edit
- [ ] Bazaar (browse / sell)
- [ ] Player ↔ player trade window — 4×2 item grid + gil selector (tab to fill
  digits / max) + per-stack quantity selector + OK/Cancel; rare/ex and equipped
  items disabled; reuses the item detail panel
- [ ] Emote animations

#### World interaction

- [x] Zone change, system messages
- [ ] NPC interaction range gate (~6 yalms): talk / trade / check fail with
  "Target out of range" beyond it; no contextual menu opens out of range
- [~] NPC dialogue — text renders; **choice branching missing** (no
  progression past the first prompt)
- [~] NPC shops — buy works; sell-to-NPC missing
- [ ] Synthesis / crafting UI
- [ ] Auction House
- [ ] Mog House furniture / safe / locker UI
- [ ] Fishing
- [ ] Chocobo riding
- [ ] Chocobo digging
- [ ] Trust / Fellow NPC

#### Character & progression

- [ ] Status menu (jobs, levels, skills, stats breakdown) — submenu: Profile,
  Job Levels, Master Levels, Combat / Magic / Craft Skill, Currencies (1 & 2),
  Unity, Play Time (→ chat), Merit Points, Job Points. Profile panel shows
  name, job/sub-job + levels, item level, HP/MP/TP, STR/DEX/VIT/AGI/INT/MND/CHR
- [ ] Magic spell list (learned spells)
- [ ] Abilities / job traits list
- [ ] Merits, job points, capacity points
- [ ] Homepoint UI
- [ ] Mission log
- [ ] Quest log
- [ ] Key items

#### Launcher / lobby

- [x] Character select (view)
- [~] Character create flow — partial; `TODO(litigate)` enumerate what
  remains (race/face/hair pickers? nation pick? confirm step?)
- [ ] Character delete flow

### Enhanced / addon (opt-in, no retail analog)

Modernization layers that intentionally diverge from the official client.
Each should be gated behind a feature flag or build flavor so vanilla
parity isn't compromised by default.

- [~] WoW / Ashita-style minimap — retail and topdown modes; self-marker
  not heading-rotated. This replaces the vanilla compass for users who
  want it.
- [~] Quick-action ring (`hud/quick_action.rs`) — routes to submenus; direct
  one-press cast not wired. This is the Enhanced addon ring, **distinct from**
  the vanilla target-action contextual menu tracked under Vanilla → HUD.
  `TODO(litigate)` confirm whether "quick action menu" is the preferred name.
- [ ] DPS meter / combat-log parser
- [ ] Skillchain visualizer
- [ ] Item compare tooltips
- [ ] Gear sets / gear-swap macros (beyond vanilla macro palette)
- [ ] Plugin / extension API (the umbrella that lets the rest of this
  section exist as community contributions)

## Reference material (`research/`)

When re-implementing a feature it helps to read how other community clients
behave. Those upstreams live under `research/` as **read-only references** —
never redistributed by this repo (gitignored or submodule pointers). Study the
behavior and re-express it in our own code; don't copy source in.

The most useful one is [XIM](https://xim.pages.dev/), a from-scratch browser
FFXI client. Fetch a local copy with the helper script (GPL-3, gitignored):

```bash
research/fetch-xim.sh
```

See [`research/README.md`](research/README.md) for the full list, sources, and
the reference-only policy.

## Contributing

Pick a `[~]` or `[ ]` line above and open a PR. When you finish a feature
(or take it from missing to partial), flip its glyph in the same commit so
the scoreboard stays honest.

Before adding a new line: decide which scoreboard it belongs on. If the
official FFXI client doesn't have it, it goes under **Enhanced / addon**.

The vanilla menu / target-interaction gaps (target-action contextual menu,
trade window, item detail panel, status/profile panel, main-menu "Commands"
ordering, NPC range gate, door zone-transition flow) are specced and staged in
[`docs/vanilla-menu-parity-plan.md`](docs/vanilla-menu-parity-plan.md).

For protocol questions, `play --headless` emits a JSON event stream that's
easy to inspect. For rendering work, the default `play` GUI window is the
fast iteration loop.
