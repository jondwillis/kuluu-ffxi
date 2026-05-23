# Kuluu

A Rust + Bevy client for the FINAL FANTASY XI protocol. Native window, chase
camera, retail-feel HUD, and a parity scoreboard tracked honestly against the
official client.

Kuluu is a **game-preservation effort**. FINAL FANTASY XI is a Square Enix
property; this repo has no affiliation with, and no endorsement from, Square
Enix. If you enjoy FFXI, please support the official service.

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
  (e.g. mounted at `vendor/Game`). Translated tables vendored from LSB,
  POLUtils, AltanaListener, etc. are stored as derived constants under the
  upstream license — never as game content. See [vendored data sources]
  below.
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
- `ffxi-client` — interactive client; subcommands `play` (headless / JSON
  event stream) and `native` (GUI window). Login, session, input, reactor.
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

Native window:

```bash
cargo run -p ffxi-client --features native-window -- native
```

Headless (text-mode session, useful for protocol work):

```bash
cargo run -p ffxi-client -- play
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
- [ ] Interactible doors (open/close, zone transition surfaces)
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
- [~] Main menu (`-`): Equipment, Magic, Abilities, Items submenus wired;
  Status, Party, Search, Macros, Graphics submenus still stubbed
- [ ] Full-screen region map, quest markers, zone-line glyphs
- [ ] Recast-timer display
- [ ] Buff / debuff duration timers
- [ ] Macro palette editor

#### Combat & action

- [x] Target lock + tab-cycle (by distance)
- [x] Auto-attack engage / disengage, engaged-target ring
- [x] Combat-stance animation
- [x] `/check` — mob con (level / defense estimate)
- [ ] `/check` on players — inspect equipment / bazaar wares.
  `TODO(litigate)` confirm this is what "item /check" meant in the old
  scoreboard
- [~] Action dispatch from menu — item use wired; magic / ability dispatch
  partial
- [ ] Weaponskill chain UI (vanilla skillchain message + animation, not the
  addon visualizer)

#### Inventory & equipment

- [~] Main inventory: decoded and browsable; sub-containers (Sack, Satchel,
  Case, Wardrobe, Mog Locker, Gobbiebag) data-ready, no per-container view
- [~] Equipment: 16 slots displayed, equip-from-inventory works — unequip
  and basic gear management missing
- [ ] Stack split, drop, sort

#### Party & social

- [x] Party state, HP / MP %, low-HP event
- [x] Chat send / receive on Say, Shout, Tell, Party, Linkshell, Yell,
  System
- [ ] Linkshell 2 + linkshell management UI
- [ ] `/search` + player search panel
- [ ] Search-comment edit
- [ ] Bazaar (browse / sell)
- [ ] Player ↔ player trade window
- [ ] Emote animations

#### World interaction

- [x] Zone change, system messages
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

- [ ] Status menu (jobs, levels, skills, stats breakdown)
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
- [~] Quick-action bar — routes to submenus; direct one-press cast not
  wired. `TODO(litigate)` confirm whether "quick action menu" is the
  preferred name.
- [ ] DPS meter / combat-log parser
- [ ] Skillchain visualizer
- [ ] Item compare tooltips
- [ ] Gear sets / gear-swap macros (beyond vanilla macro palette)
- [ ] Plugin / extension API (the umbrella that lets the rest of this
  section exist as community contributions)

## Vendored data sources

Kuluu's `vendor/` tree holds reference material that is **read-only and
gitignored** unless the upstream license allows redistribution. Examples:

- `vendor/server/` — LandSandBoat (LSB), authoritative for protocol /
  server-behavior questions.
- `vendor/xiNavmeshes/` — Recast/Detour navmeshes (GPL-2).
- `vendor/AltanaListener/` — BGM catalog, used by `ffxi-audio/build.rs` to
  generate a 220-row `(track_id, name, composer)` constant table.
- `vendor/POLUtils/` — XML mappings (e.g. ROMFileMappings → zone → map-DAT
  lookup table generated at build time).
- `vendor/Game/` — **user-provided retail install**. Never committed.

Build scripts translate these into compile-time Rust constants; no
copyrighted asset bytes leave the user's machine.

## Contributing

Pick a `[~]` or `[ ]` line above and open a PR. When you finish a feature
(or take it from missing to partial), flip its glyph in the same commit so
the scoreboard stays honest.

Before adding a new line: decide which scoreboard it belongs on. If the
official FFXI client doesn't have it, it goes under **Enhanced / addon**.

For protocol questions, the headless `play` subcommand emits a JSON event
stream that's easy to inspect. For rendering work, the native window is
the fast iteration loop.
