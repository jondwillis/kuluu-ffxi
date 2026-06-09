# Vanilla menu & target-interaction parity plan

Source: the "Ffxi features" voice note (2026-06-04), a ~31-minute walkthrough of
the retail/HorizonXI client recorded as a level-5 Black Mage in a Mog House,
enumerating UI the official client has that Kuluu does not yet match to spec.
Transcript mined for gaps and reconciled against the current code
(`ffxi-viewer-core/src/hud/`).

Reference client in the note: **HorizonXI** (classic-era jobs only — White/Black
Magic, Songs, Summoning, Blue Magic; no newer job categories).

## Headline finding

The note is overwhelmingly about the **vanilla target-action contextual menu** —
the menu that opens when you select an entity and press confirm: `Chat`, `Magic`,
`Abilities`, `Trust`, `Items`, `Trade`, `Check`. **This subsystem is effectively
absent.** What exists today (`hud/quick_action.rs`, `QuickActionPanel`) is the
*Enhanced/addon* quick-action ring — a different thing with different entries
(`Attack/Check/Talk` + `Magic/Abilities/Items/Macros`). It must not be confused
with, or substituted for, the vanilla contextual menu.

This plan therefore introduces a **new vanilla subsystem** rather than extending
the addon ring, and threads the smaller HUD/menu corrections through it.

## Current state (grounded in code)

| Area | File / symbol | State |
|---|---|---|
| Self HUD (HP/MP/TP) | `hud/self_hud.rs` — `SelfHudPanel`, `SelfHpRow/MpRow/TpRow`, `SelfStatusRow` | exists; **no solo/party indicator**; 220px |
| Target panel | `hud/target_panel.rs` — `TargetPanel` | exists; **280px (mismatch vs 220px self/party)** |
| Party panel | `hud/roster.rs` — `RosterPanel`, `BarStat` | exists; 220px |
| Quick-action ring (addon) | `hud/quick_action.rs` — `QuickActionPanel` | partial; **not the vanilla contextual menu** |
| Main menu | `hud/menu.rs` — `MenuKind`, `MainMenu`, `MenuRowActivated` | exists; order/contents diverge from retail |
| Magic submenu | `MenuKind::Magic`, `DynamicMenuAction::CastSpell` | partial; **no White/Black/Song/Summon/Blue categories** |
| Abilities submenu | `MenuKind::Abilities`, `DynamicMenuAction::{JobAbility,Weaponskill,PetAbility}` | partial; **no Ranged/Mount grouping, no contextual error strings** |
| Trade window | — | absent |
| Item detail / tooltip | — | absent |
| `/check` player (wares + 4×4 gear) | — | absent |
| Status / profile panel | `MenuKind::Status` (stub) | absent (panel) |
| Item sort (Auto/Manual/Recycle) | — | absent (no inventory UI) |
| NPC talk/trade range gate | — | **no ~6-yalm constant** |
| Door zone-transition dialog | `hud/zone_flash.rs` — `ZoneFlashBanner` | partial; banner only — no yes/no, no "downloading data", no interactive fade |
| Trust / Fellow | — | absent |

## Spec details extracted from the note

### A. Self / target / party panels
- **Solo indicator** belongs *inside* the bottom-right self panel, alongside the
  player name and the HP/MP (and where applicable other-resource) bars.
- **Width parity**: the target panel width must match the self/party panel width.
  Today target is 280px, self/party are 220px.

### B. Target-action contextual menu (the spine)
Opens on confirm with a target selected. Entries observed (self-targeted, lvl-5 BLM):

1. **Chat** — *select button* (right-arrow cycles its mode). Right cycles
   `Say → Tell → Party → Linkshell → Unity → Shout`. Confirm (without cycling)
   opens chat to the selected entity, defaulting to `/tell <targetname>`; only
   valid for **player characters**.
2. **Magic** — *select button*. Confirm opens the flat list of currently-castable
   spells (e.g. lvl-5 BLM: Stone, Water, Poison). Right opens a **category panel**:
   `White Magic / Black Magic / Songs / Summoning / Blue Magic` (classic set).
   Selecting a category lists its spells or shows **"No spells available."**
3. **Abilities** — *plain button*. Confirm replaces the contextual menu with:
   `Job Abilities / Weapon Skill / Ranged Attack / Mount / Pet Command`. Each
   resolves to a list **or a contextual error**:
   - Job Abilities → e.g. "Manafont" (+ weaponskills), else "No abilities available."
   - Ranged Attack → "You cannot use that command here." (in Mog House / no ranged weapon)
   - Mount → "No mounts available."
   - Pet Command → "No abilities available."
4. **Trust** — present in retail; **not implemented** here (no trust subsystem).
5. **Items** — confirm replaces the contextual menu with the usable-items list.
   - Top-left tooltip: `Items` + count as `usable / total` (note: denominator
     counts items present even if not usable — observed `11` shown vs `14`).
   - Helper text to the right: **"Select an item."** (appears/disappears contextually).
   - Bottom-left **item detail panel**, docked to the top of the chat box,
     **replaces the compass + clock** while open (see D).
6. **Trade** — contextual: shown for non-mob, non-door interactables (PC + NPC).
   Opens a sub-target indicator; out-of-range → "Target out of range." See C.
7. **Check** — on a PC, opens the **bazaar + equipment** view (see E). Chat emits
   "`<name>` examines you." (contextual to who is examining).

Notes:
- Backing out of any sub-action returns to the contextual menu; backing out of the
  contextual menu returns to world.
- No contextual menu appears when the action resolves directly to chat, or when the
  target is out of range (the action is rejected with a message instead).

### C. Trade window
- Title "Trade", no helper text. **4 columns × 2 rows = 8 item slots.**
- Up from the grid selects **Gil** → a gil-amount selector that also shows current
  gil; tabbing left fills digits, tabbing past the digit count sets the **max**
  (= current gil). Confirm sets the traded gil; re-entering resets to 0.
- **OK** sits in the first cell alongside the empty item slots; **Cancel** is
  directly below OK. Escape selects Cancel but does not exit; Cancel must be
  confirmed to leave.
- Item picking pulls from inventory; **disabled** entries: rare/ex and currently
  **equipped** items. Placing an item paints the slot **reddish-orange** and marks
  the inventory row likewise; re-confirming a placed item clears it.
- **Stackable** items open a stack selector: pick `1..max` (max = current count,
  up to 99); up/down adjust, right jumps to max, down past 1 is a no-op. Stacks
  move all-or-nothing into a slot.
- A tooltip under the item list shows the focused item's name + description.

### D. Item detail / tooltip panel
Docked bottom-left (replaces compass/clock while the items list is open). Fields:
- Icon, name, rare/ex icons.
- Equipment: slot ("Waist"), race/job restriction, level ("Lv.1 All Jobs"),
  enchantment line + icon, **uses remaining** (`9/10`) and **cooldown** (e.g.
  `0:00 / (1:00, 15s)` — recast vs. duration).
- Consumable: description + **status effect granted** ("HP +10, MP +10") + duration
  ("30 min").

### E. `/check` on a player → wares + gear
- Tooltip: player name + "Lv.5 Black Mage" (level + job).
- Focus starts on **View Wares** (bazaar — absent today). Empty bazaar skips
  straight into the gear grid.
- **4×4 equipment grid**, slot adjacency (from the note): row layout around
  Main/Sub, Range/Ammo, Head/Neck/Ear1/Ear2, Body/Hands/Ring1/Ring2,
  Back/Waist/Legs/Feet. Focusing a slot shows its item detail (same panel as D).
- Resolves the old `TODO(litigate)` on "item /check": yes — `/check` on a PC is the
  bazaar-wares + equipment inspector.

### F. Main menu ("Commands")
Title is **"Commands"**. Two-column layout; right-arrow toggles columns, preserving
the row index. Retail order:

- Column 1: `Status, Equipment, Magic, Items, Synthesis, Abilities, Party, Trade, Search, Linkshell, Region Info, Map`
- Column 2: `Missions, Quests, Key Items, View House, Macros, Config, Help Desk, Time, Communication, Shut Down, Log Out`

Current `MenuKind::Root` order/contents diverge and omit many entries; align order,
title, and the two-column toggle.

### G. Status submenu / profile panel
Selecting **Status** opens a submenu (title "Status"); the **Profile** entry shows
help "View your profile including current allegiance and title" and renders a
top-left profile panel:
- Name, job + level, sub-job + level (omitted line if none), item level, HP/MP/TP,
  STR/DEX/VIT/AGI/INT/MND/CHR.

Submenu entries: `Profile, Job Levels, Master Levels (disabled), Combat Skill,
Magic Skill, Craft Skill, Currencies, Currencies 2, Unity, Play Time,
Merit Points (disabled), Job Points`.
- **Play Time** emits to chat: "Total time played is 21 hours 58 minutes 3 seconds"
  (server-sourced).

### H. Items menu (from main menu) + sort
- Top-left lists all items + equipment (icons, stacks); count `14 / 30`
  (held / capacity); helper "Select an item."
- Bottom-left **Options** panel, label "Sort", three choices:
  - **Auto** → confirm yes/no → server/auto sort, refocuses the item list.
  - **Manual** → manual swap of item positions (**not implemented** in retail-note's
    own client either — low priority).
  - **Recycle Bin** → recently discarded items (**not implemented** upstream — low priority).

### I. NPC interaction range
- Talk range gate is **~6 yalms** (note measured 5.9 ok, 6.0 not). Out of range →
  "Target out of range" and **no** contextual menu opens.
- Trade uses the **same ~6-yalm** gate (sub-target turns red within range).

### J. Door / zone-transition interaction
- Selecting a door opens a yes/no dialog ("Moogle heading outside, kupo?" style;
  **No** default-selected), right of the compass/clock/weekday cluster.
- Confirm → door open animation → fade to black → **"Downloading data"** indicator
  bottom-right → fade in → zone changed.
- Ties into the existing `[ ] Interactible doors` line and the partial NPC-dialogue
  **choice-branching** gap.

## Implementation plan (staged)

Ordering favors the spine first, then the leaf views that reuse it.

### Stage 0 — Foundations & cheap wins
- **0.1** Width parity: make `TargetPanel` share the self/party width constant
  (introduce a single `PANEL_WIDTH` in `hud/`); target panel 280→220.
- **0.2** Solo/party indicator in `SelfHudPanel` (new row/badge in `self_hud.rs`).
- **0.3** Introduce an `NPC_INTERACT_YALMS = 6.0` constant and gate talk/trade/
  check actions on it, emitting "Target out of range" otherwise.

### Stage 1 — Vanilla target-action contextual menu (new subsystem)
- New module `hud/target_action_menu.rs` distinct from `quick_action.rs`:
  - `TargetActionMenu` panel, `TargetActionRow`, `TargetActionActivated` message.
  - New `InputMode::TargetAction` (sibling to existing `QuickAction`); confirm-on-
    target opens it; back closes it.
  - Entry model supporting two kinds: **plain button** vs **select button**
    (right-arrow cycles/expands).
  - Contextual entry visibility (Chat/Trade/Check only for valid PC/NPC targets;
    Trust hidden until trusts exist).
- Chat entry (B.1): mode cycle + `/tell <target>` default; reuse existing chat send.

### Stage 2 — Magic & Abilities sub-actions (reuse `DynamicMenu`)
- **2.1** Magic categories (B.2): add a category layer
  (White/Black/Song/Summon/Blue) above the existing `CastSpell` list; "No spells
  available" empty state. Drives both the contextual menu and `MenuKind::Magic`.
- **2.2** Abilities grouping (B.3): `Job Abilities / Weapon Skill / Ranged Attack /
  Mount / Pet Command` with contextual error strings ("You cannot use that command
  here.", "No mounts available.", "No abilities available."). Reuses
  `DynamicMenuAction::{JobAbility,Weaponskill,PetAbility}`; add Ranged/Mount paths.

### Stage 3 — Item detail panel + Items list
- **3.1** `hud/item_detail.rs` — `ItemDetailPanel` docked bottom-left, hiding the
  compass/clock while open (D). Pulls from decoded inventory + an item-metadata
  source (rare/ex, slot, jobs, level, enchantment, uses/recast, status-grant).
- **3.2** Wire the contextual **Items** entry (B.5) and the main-menu Items list
  (H) to it; `usable/total` and `held/capacity` counts; "Select an item." helper.
- **3.3** Item **sort**: Auto (yes/no → sort) now; Manual/Recycle Bin stubbed
  (parity-optional, upstream-incomplete).

### Stage 4 — Trade window
- `hud/trade.rs` — `TradeWindow`: 4×2 grid + gil selector + stack selector +
  OK/Cancel (C). Drives `0x036 TRADE_REQUEST` flow. Reuses item detail panel and
  the rare/ex/equipped disable rules.

### Stage 5 — `/check` player view + Status/profile panel
- **5.1** `hud/check_view.rs` — View Wares (bazaar stub) + 4×4 equipment grid
  reusing the item detail panel (E). Emits "`<name>` examines you."
- **5.2** `hud/status_panel.rs` — profile panel + Status submenu entries (G),
  including Play Time → chat. Replaces the `MenuKind::Status` stub.

### Stage 6 — Main menu alignment
- Reorder/rename `MenuKind::Root` to the retail "Commands" two-column layout (F);
  add missing entries (Synthesis, Trade, Search, Linkshell, Region Info, Map,
  Missions, Quests, Key Items, View House, Help Desk, Time, Communication,
  Shut Down) as stubs that flip to real submenus as those land.

### Stage 7 — Door zone-transition dialog
- Extend `zone_flash.rs` / dialog system: door yes/no confirm, open animation hook,
  "Downloading data" indicator, interactive fade (J). Couples with NPC-dialogue
  choice-branching work.

## Out of scope / deferred
- **Trust / Fellow** subsystem (entry hidden until it exists).
- Manual item sort + Recycle Bin (incomplete even in the reference client).
- Bazaar selling (View Wares renders; sell flow is a separate inventory feature).

## README scoreboard impact
See the companion edits to `README.md`:
- New **Vanilla** line for the target-action contextual menu (distinct from the
  Enhanced quick-action ring).
- Self-HUD solo indicator + target-panel width-parity notes.
- Magic categories, Abilities grouping, item detail panel, NPC interaction range,
  door zone-transition dialog, main-menu "Commands" ordering, Status/profile panel,
  item sort detail.
- Resolve the `/check`-on-players `TODO(litigate)` (confirmed: wares + gear).
