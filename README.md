# Kuluu

A Rust + Bevy client for the FINAL FANTASY XI protocol. Aimed squarely at the
human player: chase camera, native window, retail-feel HUD, and a parity
scoreboard that tracks how close it is to feature-complete.

The project is a **game-preservation effort**. FINAL FANTASY XI is a Square
Enix property; this repo has no affiliation with, and no endorsement from,
Square Enix. If you enjoy FFXI, please support the official service.

## What this is — and isn't

- **Is**: a from-scratch FFXI-protocol client, native-first, built around
  modern tooling (Bevy, wgpu, Tokio).
- **Is not**: a tool for retail. The retail FFXI service has anti-cheat and
  Terms of Service Kuluu makes no attempt to honor; do not point it there.
- **Is not**: a server. Kuluu speaks to community-run FFXI-protocol servers.
  Each such server has its own rules — read them before you log in.

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

If any credential env var is unset, the launcher will prompt for it and list
characters on the account so you can pick by name.

## Retail-parity scoreboard

A living checklist of where Kuluu stands relative to the retail FFXI
experience. Contributions are welcome on any incomplete line — pick something
you'd want to play with.

Legend: `[x]` implemented · `[~]` partial (decoded or scaffolded, UI or
dispatch incomplete) · `[ ]` missing · `[?]` not investigated.

### Rendering & world

- `[x]` Scene graph, entity sync, FFXI→Bevy coord transform
- `[x]` PC + NPC skinned meshes
- `[x]` Skybox dome with sRGB-correct lerp
- `[x]` Sun + moon billboards
- `[x]` Weather visual FX
- `[x]` Music playback, per-channel mute
- `[~]` Sound effects — scheduler runs, action→sound table not yet mapped
- `[x]` Picking, hover-cursor, entity hover card
- `[x]` Chase camera + collision
- `[~]` Nameplate billboards — distance-to-scale curve still placeholder
- `[~]` Minimap — retail and topdown modes; self-marker not heading-rotated
- `[ ]` Full-screen region map, quest markers, zone-line glyphs

### HUD

- `[x]` Stage bar, chat panes (Social/Battle/Debug), self HUD, party roster,
  target panel, Vana clock, compass, weather icon, zone flash, dialog window,
  death prompt, logout countdown, status ribbon
- `[~]` Main menu (`-`): Equipment, Magic, Abilities, Items submenus wired;
  Status, Party, Search, Macros, Graphics submenus still stubbed
- `[~]` Quick-action bar — routes to submenus; direct one-press cast not wired
- `[ ]` Recast-timer display
- `[ ]` Buff/debuff duration timers
- `[ ]` Macro palette editor

### Combat & action

- `[x]` Target lock + tab-cycle by distance
- `[x]` Auto-attack engage/disengage, engaged-target ring
- `[x]` Combat-stance animation
- `[x]` `/check` decoder
- `[~]` Action dispatch from menu — item use wired; magic/ability dispatch partial
- `[ ]` Weaponskill chain UI, skillchain visualizer
- `[ ]` DPS meter / combat-log parser

### Inventory & equipment

- `[~]` Main inventory: decoded and browsable; sub-containers (Sack, Satchel,
  Case, Wardrobe, Mog Locker, Gobbiebag) data-ready, no per-container view
- `[~]` Equipment: 16 slots displayed, equip-from-inventory works — unequip,
  gear sets, gear-swap macros missing
- `[ ]` Item compare tooltips, item `/check`
- `[ ]` Stack split, drop, sort

### Party & social

- `[x]` Party state, HP/MP %, low-HP event
- `[x]` Chat send/receive on Say, Shout, Tell, Party, Linkshell, Yell, System
- `[ ]` Linkshell 2 + linkshell management UI
- `[ ]` `/search` + player search panel
- `[ ]` Search-comment edit
- `[ ]` Bazaar (browse/sell)
- `[ ]` Player↔player trade window
- `[ ]` Emote animations

### World interaction

- `[x]` Zone change, system messages
- `[~]` NPC dialogue — text renders, **choice branching missing** (no
  progression past the first prompt)
- `[~]` NPC shops — buy works, sell-to-NPC missing
- `[ ]` Synthesis / crafting UI
- `[ ]` Auction House
- `[ ]` Mog House furniture / safe / locker UI
- `[ ]` Fishing
- `[ ]` Chocobo / mounts
- `[ ]` Trust / Fellow NPC

### Character & progression

- `[ ]` Status menu (jobs, levels, skills, stats breakdown)
- `[ ]` Merits / job points / homepoint UI
- `[ ]` Trait list, key items, mission log, quest log

### Launcher / lobby

- `[x]` Character select (view)
- `[ ]` Character create flow
- `[ ]` Character delete flow

## Contributing

Pick a `[~]` or `[ ]` line above and open a PR. When you finish a feature
(or take it from missing to partial), flip its glyph in the same commit so
the scoreboard stays honest. There is no roadmap beyond the scoreboard —
work on what you want to play.

For protocol questions, the headless `play` subcommand emits a JSON event
stream that's easy to inspect; for rendering work, the native window is the
fast iteration loop.
