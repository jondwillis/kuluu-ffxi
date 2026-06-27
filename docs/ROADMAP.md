# Roadmap & parity scoreboard

Kuluu tracks its progress honestly against the official FINAL FANTASY XI
client. This file is a **directional overview** — the broad shape of what's
done and what's left. The granular, actionable parity backlog is the **system
of record in beads** (`bd ready`; label `roadmap`), where each line has been
grounded against the code with `file:line` evidence. Keep the glyphs below
roughly honest as those beads open and close, but claim and track work in
beads, not here.

There are **two** scoreboards:

- **Vanilla parity** — features that exist in the official client. The aim is
  close to 1:1 with retail in default mode: same menus, same compass, same
  combat feel.
- **Enhanced / addon** — opt-in modernization layers with no retail analog
  (WoW-style minimap, DPS meter, gear swap, …). Each should be gated behind a
  feature flag or build flavor so vanilla parity isn't compromised by default.

A feature only ever lives on one of the two lists. Before adding a new line,
decide which scoreboard it belongs on: if the official FFXI client doesn't have
it, it goes under **Enhanced / addon**.

**Legend:** `[x]` implemented · `[~]` partial (decoded or scaffolded, UI or
dispatch incomplete) · `[ ]` missing · `[?]` not verified from code (needs a
runtime/visual check).

> **Last verified: 2026-06-12** — a *code-level* audit (presence and wiring of
> systems/panels/dispatch) plus a *headless-render* grounding pass for the
> world items (`zone-render-headless`, `actor-render-headless` against a real
> retail install). Items whose real state can only be confirmed in-game
> (multi-step flows, in-zone interiors) are noted as such. If you touch an
> area, re-verify the line you change.

---

## Vanilla parity

### World & rendering

- [~] Zone geometry — MZB/MMB meshes load with the FFXI→Bevy coord transform
  and render through the faithful vertex-lit `FfxiZoneMaterial`. Headless
  top-down renders of Bastok Markets, Southern San d'Oria, and East
  Sarutabaruta show floors, streets, walls, water, and terrain rendering with
  textures. Full per-zone coverage is unverified — some zones may still have
  gaps that only show in-zone:
  - [~] Bastok Markets — pavement/buildings render in the headless top-down
    capture; the old "missing floors" report was not reproduced there, but
    needs an in-zone confirmation pass
  - [~] Southern San d'Oria — floors/walls/plazas render fully in the headless
    capture; "missing interior houses" not reproduced top-down (interiors
    still need an in-zone check)
  - [ ] `TODO(litigate)` sweep the remaining zones in-game and enumerate any
    that still render with holes
- [x] PC + NPC skinned meshes — NPC models render full-body, posed, and
  textured in `actor-render-headless`; textures are mip-mapped + anisotropic
  (no whole-body minification shimmer under camera motion); entities snap to the
  MZB collision floor via nearest-floor grounding (no per-frame Y-bob)
- [ ] Airships, boats, elevators — `EntityLook::Transport` is decoded but
  skipped in the render dispatch; no moving-platform logic yet
- [ ] Interactible doors (open/close, zone-transition surfaces) —
  `EntityLook::Door` decoded but skipped in dispatch. The retail door flow
  (yes/no confirm → open animation → fade-to-black → "Downloading data" →
  fade-in → zone changed) is unimplemented
- [x] Skybox dome with sRGB-correct lerp
- [~] Sun + moon — billboards render; altitude arc and moon phase computed;
  color tint and Vana-day timing not verified against retail
- [x] Weather visual FX
- [~] Dynamic point lights — faithful MZB Generator lights + `/lights` over-bright
  vertex emitters feed the custom materials; gated to dusk→dawn off the Vana
  clock, windowed falloff to zero at range (no pop-in), reach scaled by the
  Graphics "Range" knob. Per-zone lamp coverage not audited
- [x] Music playback, per-channel mute
- [~] Sound effects — scheduler + system-event SFX table run; the
  action→sound table is not yet hand-mapped
- [x] Picking, hover-cursor, entity hover card
- [x] Chase camera + collision

### HUD

- [~] Retail compass (minimal entity-direction widget)
- [~] Nameplate billboards — render; distance-to-scale curve still placeholder
- [x] Stage bar, chat panes (Social / Battle / Debug), self HUD, party
  roster, target panel, Vana clock, weather icon, zone flash, dialog
  window, death prompt, logout countdown, status ribbon
- [x] Self HUD solo / party indicator
- [x] Target panel width parity — self/party/target panels share `PANEL_WIDTH_PX`
- [~] Target-action contextual menu (vanilla) — select an entity → confirm
  opens Attack / Chat / Magic / Abilities / Trust / Items / Trade / Check, with
  per-entry contextual visibility. Engaged in battle swaps Attack → Switch
  Target (an `<st>`-style select cursor: Tab-cycle + re-engage) and adds
  Disengage + Items; docked above the chat log; submenu entries render a ▶
  arrow. Compass / Vana clock relocated beside the minimap
- [x] Item detail / tooltip panel (docked bottom-left): icon, name, slot,
  race/job, level, recast, uses-remaining
- [~] Main menu (`-`), title "Commands": Equipment / Status / Graphics /
  Config wired; Magic / Abilities / Items submenus show "data pending";
  Party / Search / Macros not yet routed. Order/contents still diverge from
  retail's two-column layout
- [ ] Full-screen region map, quest markers, zone-line glyphs (3D zone-line
  rendering exists; no map HUD)
- [~] Recast-timer display (job-ability recast from 0x119 shown + greyed in the
  abilities menu; spell/item recast not yet)
- [~] Buff / debuff duration timers (status ribbon shows self-effect countdowns
  from 0x063 timestamps; party/target durations not yet)
- [ ] Macro palette editor

### Combat & action

- [x] Target lock + tab-cycle (by distance) — dead and LSB-hidden entities
  (STATUS_TYPE not NORMAL/UPDATE) are excluded from the cycle and from
  click-selection; dead players stay click-targetable (for Raise)
- [x] Auto-attack engage / disengage, engaged-target ring
- [x] Combat-stance animation
- [x] `/check` — mob con (level / defense estimate)
- [~] `/check` on players — "View Wares" + 16-slot (4×4) equipment grid
  inspector
- [x] Action dispatch from menu — item / magic / ability all dispatch via the
  target-action contextual menu
- [~] Magic list spell categories (White / Black / Songs / Summoning / Blue) —
  category model + dispatch present; per-category "No spells available"
  empty-state not confirmed
- [~] Abilities sub-grouping (Job Abilities / Weapon Skill / Ranged / Mount /
  Pet Command) — groups + some contextual error strings present
- [~] Weaponskill chain UI — weaponskill dispatch exists; the vanilla
  skillchain message + animation display is not confirmed
- [~] Spell / ability completion effects — the effect DAT (resolved by
  spell/ability id) runs its `main` scheduler; a CPU particle emission VM
  (`ffxi-dat::particle_gen`, `ffxi-viewer-core::particle_sim`) streams
  billboard particles from the generators with keyframe scale/alpha curves
  and per-generator blend mode. Shared/global billboard meshes and sec3/sec4
  updaters (rotation, oscillation, child generators) are still gaps (kuluu-8i3)

### Inventory & equipment

- [~] Main inventory: main bag decoded and browsable; sub-containers (Sack,
  Satchel, Case, Wardrobe, Mog Locker, Gobbiebag) data-ready, no per-container
  view
- [~] Equipment: 16 slots displayed, equip-from-inventory works; unequip and
  broader gear management not confirmed
- [~] Stack split, drop, sort — sort menu (Auto / Manual / Recycle Bin) and the
  held/capacity count display exist; split/drop dispatch not confirmed

### Party & social

- [x] Party state, HP / MP %, low-HP event
- [x] Chat send / receive on Say, Shout, Tell, Party, Linkshell, Yell, System
- [~] Linkshell 2 — parsed but folded into the single Linkshell channel; no
  LS2 separation or linkshell-management UI
- [ ] `/search` + player search panel
- [ ] Search-comment edit
- [ ] Bazaar (browse / sell)
- [~] Player ↔ player trade window — 4×2 grid + gil selector + quantity +
  OK/Cancel
- [ ] Emote animations

### World interaction

- [x] Zone change, system messages
- [x] NPC interaction range gate (~6 yalms): talk / trade gated with a range
  check (`NPC_INTERACT_YALMS`). Check (examine) is intentionally not range-gated
  — the server resolves it for any targetable entity in awareness range
- [~] NPC dialogue — text renders and choice buttons + `DialogChoiceActivated`
  are wired; multi-step branching progression needs a runtime check
- [~] NPC shops — buy works; sell-to-NPC not confirmed
- [ ] Synthesis / crafting UI
- [ ] Auction House
- [ ] Mog House furniture / safe / locker UI (mog-house exit exists; no storage UI)
- [~] Fishing — `ActionKind::Fish` dispatch exists; no fishing UI / minigame
- [~] Chocobo riding — `ActionKind::Mount` dispatch exists; no riding UX
- [~] Chocobo digging — `ActionKind::ChocoboDig` dispatch exists; no dig UI
- [~] Trust / Fellow NPC — Trust entry in the target-action menu, gated by a
  `trusts_available` flag; no trust list / management UI

### Character & progression

- [~] Status menu — Profile, Job Levels, Combat/Magic/Craft Skill, Currencies,
  Unity, Play Time, Job Points wired; Master Levels + Merit Points disabled.
  Profile panel shows name, job/sub-job + levels, item level, HP/MP/TP,
  STR/DEX/VIT/AGI/INT/MND/CHR
- [x] Magic spell list (learned spells) — `spells_known` renders learned
  spells by name with cast actions; spell categories tracked separately
- [x] Abilities / job traits list — `job_abilities_known` / `weaponskills_known`
  / `pet_abilities_known` render by name with activation actions; sub-grouping
  tracked separately
- [ ] Merits, job points, capacity points (not in wire/session state)
- [~] Homepoint UI — `/homepoint`, `/homepointmenu` slash commands exist; no
  interactive homepoint menu
- [ ] Mission log
- [ ] Quest log
- [ ] Key items

### Launcher / lobby

- [x] Character select (view)
- [x] Character create flow — race / job / nation / size / face pickers + name
  entry wired
- [x] Character delete flow — delete confirmation dialog wired

---

## Enhanced / addon (opt-in, no retail analog)

Modernization layers that intentionally diverge from the official client.
Each should be gated behind a feature flag or build flavor so vanilla
parity isn't compromised by default.

- [~] WoW / Ashita-style minimap — retail and topdown modes (gated
  `cfg(not(target_arch = "wasm32"))`). Retail map AABB is calibrated from the
  FFXiMain.dll ZoneMapTable (falls back to the top-down bake AABB). Remaining:
  self-marker not heading-rotated; retail DXT / flag-`0xA1` map images aren't
  decoded yet, so those zones fall back to TopDown. Replaces the vanilla
  compass for users who want it.
- [x] Render scale (`graphics::render_scale`, `/renderscale`) — render the 3D
  scene to an off-screen image at a fraction (perf) or multiple (SSAA) of the
  window resolution and upscale-composite it to the window; the HUD stays at
  native resolution. Cross-platform (Metal/Vulkan/DX), unlike Bevy 0.18's
  RTX+Vulkan-only DLSS, so it serves the macOS and AMD Steam Deck targets.
  Default 100% is the native no-op path, so vanilla parity is preserved without a
  feature flag. In: bilinear upscale + a synthetic-pointer picking bridge so
  click-to-target works below 100%. Next: FSR1 (EASU+RCAS) WGSL upscale pass.
- [~] Quick-action ring (`hud/quick_action.rs`) — fires a slot-activation
  message; submenu routing happens at the client layer; direct one-press cast
  not wired. Distinct from the vanilla target-action contextual menu above.
- [ ] DPS meter / combat-log parser
- [ ] Skillchain visualizer
- [ ] Item compare tooltips (single-item detail exists; no side-by-side compare)
- [ ] Gear sets / gear-swap macros (beyond vanilla macro palette)
- [ ] Plugin / extension API (the umbrella that lets the rest of this
  section exist as community contributions)

---

The vanilla menu / target-interaction gaps that remain (main-menu "Commands"
ordering, Magic/Abilities list views, region map, recast/buff timers, macro
editor, door zone-transition flow) are specced and staged in
[`vanilla-menu-parity-plan.md`](vanilla-menu-parity-plan.md).
