# Vanilla menu & target-interaction spec (retail FFXI, observed on HorizonXI)

## Contents
- Target-action contextual menu
- Trade window
- Item detail / tooltip panel
- `/check` on a player -> wares + gear
- Main menu ("Commands")
- Status submenu / profile panel
- Items menu (from main menu) + sort
- NPC interaction range
- Door / zone-transition interaction

Observed 2026-06-04 via a ~31-minute walkthrough recorded as a level-5 Black Mage
in a Mog House. Reference client: HorizonXI (classic-era jobs only — White/Black
Magic, Songs, Summoning, Blue Magic; no newer job categories).

Kuluu implemented this spec in `hud/target_action_menu.rs`, `hud/item_detail.rs`,
`hud/trade.rs`, `hud/check_view.rs`, `hud/status_panel.rs`, and `hud/menu.rs`.
This file is kept as the **observation record** — the retail behavior it describes
is the oracle to re-check against when those surfaces change. Open work is in
beads (`bd list --label=hud`), not here.

## Target-action contextual menu

Opens on confirm with a target selected. Entries observed (self-targeted, lvl-5 BLM):
`Chat`, `Magic`, `Abilities`, `Trust`, `Items`, `Trade`, `Check`.

This is **not** the Enhanced/addon quick-action ring (`Attack/Check/Talk` +
`Magic/Abilities/Items/Macros`) — they are separate subsystems with different entries.

1. **Chat** — *select button*. Right-arrow cycles `Say → Tell → Party → Linkshell
   → Unity → Shout`. Confirm without cycling opens chat to the selected entity,
   defaulting to `/tell <targetname>`; valid only for player characters.
2. **Magic** — *select button*. Confirm opens the flat list of currently-castable
   spells (lvl-5 BLM: Stone, Water, Poison). Right opens a category panel:
   `White Magic / Black Magic / Songs / Summoning / Blue Magic`. Selecting a
   category lists its spells or shows **"No spells available."**
3. **Abilities** — *plain button*. Confirm replaces the contextual menu with
   `Job Abilities / Weapon Skill / Ranged Attack / Mount / Pet Command`. Each
   resolves to a list or a contextual error:
   - Job Abilities → e.g. "Manafont" (+ weaponskills), else "No abilities available."
   - Ranged Attack → "You cannot use that command here." (in Mog House / no ranged weapon)
   - Mount → "No mounts available."
   - Pet Command → "No abilities available."
4. **Trust** — present in retail.
5. **Items** — confirm replaces the contextual menu with the usable-items list.
   - Top-left tooltip: `Items` + count as `usable / total`. The denominator counts
     items present even if not usable — observed `11` shown vs `14`.
   - Helper text to the right: **"Select an item."** (appears/disappears contextually).
   - Bottom-left item detail panel, docked to the top of the chat box, **replaces
     the compass + clock** while open.
6. **Trade** — contextual: shown for non-mob, non-door interactables (PC + NPC).
   Opens a sub-target indicator; out of range → "Target out of range."
7. **Check** — on a PC, opens the bazaar + equipment view. Chat emits
   "`<name>` examines you." (contextual to who is examining).

Backing out of any sub-action returns to the contextual menu; backing out of the
contextual menu returns to world. No contextual menu appears when the action
resolves directly to chat, or when the target is out of range (the action is
rejected with a message instead).

## Trade window

- Title "Trade", no helper text. **4 columns × 2 rows = 8 item slots.**
- Up from the grid selects **Gil** → a gil-amount selector that also shows current
  gil; tabbing left fills digits, tabbing past the digit count sets the max
  (= current gil). Confirm sets the traded gil; re-entering resets to 0.
- **OK** sits in the first cell alongside the empty item slots; **Cancel** is
  directly below OK. Escape selects Cancel but does not exit; Cancel must be
  confirmed to leave.
- Item picking pulls from inventory. **Disabled** entries: rare/ex and currently
  equipped items. Placing an item paints the slot **reddish-orange** and marks the
  inventory row likewise; re-confirming a placed item clears it.
- **Stackable** items open a stack selector: pick `1..max` (max = current count, up
  to 99); up/down adjust, right jumps to max, down past 1 is a no-op. Stacks move
  all-or-nothing into a slot.
- A tooltip under the item list shows the focused item's name + description.

## Item detail / tooltip panel

Docked bottom-left, replacing compass/clock while the items list is open. Fields:
- Icon, name, rare/ex icons.
- Equipment: slot ("Waist"), race/job restriction, level ("Lv.1 All Jobs"),
  enchantment line + icon, uses remaining (`9/10`), and cooldown
  (e.g. `0:00 / (1:00, 15s)` — recast vs. duration).
- Consumable: description + status effect granted ("HP +10, MP +10") + duration
  ("30 min").

## `/check` on a player → wares + gear

- Tooltip: player name + "Lv.5 Black Mage" (level + job).
- Focus starts on **View Wares** (bazaar). An empty bazaar skips straight into the
  gear grid.
- **4×4 equipment grid**, slot adjacency: rows around Main/Sub, Range/Ammo,
  Head/Neck/Ear1/Ear2, Body/Hands/Ring1/Ring2, Back/Waist/Legs/Feet. Focusing a
  slot shows its item detail (same panel as above).

## Main menu ("Commands")

Title is **"Commands"**. Two-column layout; right-arrow toggles columns, preserving
the row index. Retail order:

- Column 1: `Status, Equipment, Magic, Items, Synthesis, Abilities, Party, Trade,
  Search, Linkshell, Region Info, Map`
- Column 2: `Missions, Quests, Key Items, View House, Macros, Config, Help Desk,
  Time, Communication, Shut Down, Log Out`

## Status submenu / profile panel

Selecting **Status** opens a submenu (title "Status"); the **Profile** entry shows
help "View your profile including current allegiance and title" and renders a
top-left profile panel: name, job + level, sub-job + level (line omitted if none),
item level, HP/MP/TP, STR/DEX/VIT/AGI/INT/MND/CHR.

Submenu entries: `Profile, Job Levels, Master Levels (disabled), Combat Skill,
Magic Skill, Craft Skill, Currencies, Currencies 2, Unity, Play Time,
Merit Points (disabled), Job Points`.

**Play Time** emits to chat: "Total time played is 21 hours 58 minutes 3 seconds"
(server-sourced).

## Items menu (from main menu) + sort

- Top-left lists all items + equipment (icons, stacks); count `14 / 30`
  (held / capacity); helper "Select an item."
- Bottom-left **Options** panel, label "Sort", three choices:
  - **Auto** → confirm yes/no → server/auto sort, refocuses the item list.
  - **Manual** → manual swap of item positions. Not implemented in the reference
    client either.
  - **Recycle Bin** → recently discarded items. Not implemented upstream.

## NPC interaction range

Talk range gate is **~6 yalms** (measured: 5.9 ok, 6.0 not). Out of range →
"Target out of range" and **no** contextual menu opens. Trade uses the same ~6-yalm
gate (the sub-target turns red within range).

## Door / zone-transition interaction

- Selecting a door opens a yes/no dialog ("Moogle heading outside, kupo?" style;
  **No** default-selected), right of the compass/clock/weekday cluster.
- Confirm → door open animation → fade to black → **"Downloading data"** indicator
  bottom-right → fade in → zone changed.
