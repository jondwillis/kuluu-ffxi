# Retail Mog House menu — observed on HorizonXI (75-era), 2026-07-17

## Contents
- How the menu opens
- Main menu (7 entries, exact order)
- Storage submenu (15 entries, exact order)
- Delivery Box submenu — send flow, receive flow
- Change Jobs submenu
- Gardening
- Home-nation Mog House — including Delivery Box receive in the has-mail state
- Capture index

Character: Oldman (Elvaan BST47/THF), inside his **rent-a-room** in Upper Jeuno
(zone name stays "Upper Jeuno" inside the MH — confirms kuluu MH-lifecycle note).
Game runs under Ashita v4.2.0.1 in the Parallels VM; captures in this directory.

## How the menu opens

Target the **Moogle** NPC (green name, targetable with Tab — careful: the exit
Door "Back to Town" is also in the Tab cycle) and press Enter.
Moogle chat flavor line on opening Change Jobs: `Moogle : Chaaange...job! Kupopopooo!`
On closing the menu: `Moogle : I'm back, kupo~~~.`

Top help bar shows `<ItemTitle> | <help text>` for the highlighted entry,
updating per item. Esc backs out one level; esc at top level closes the menu.
Cursor position in the main menu is retained while the interaction lasts, and
was retained across a close+reopen of the menu in the same session.

## Main menu (7 entries, exact order)

| # | Label | Help text | Notes |
|---|-------|-----------|-------|
| 1 | Storage | Check all items in your Mog Safe and other storage systems. | opens container list |
| 2 | Delivery Box | Use the delivery system. | opens Receive/Send |
| 3 | Change Jobs | Change jobs. | opens Main Job/Support Job |
| 4 | Gardening | Grow plants in flowerpots. | opens furnishing-source picker |
| 5 | Layout | Rearrange the furniture in your Mog House. | **grayed/disabled in rent-a-room** (non-home-nation MH) |
| 6 | Open Mog House | Open your Mog House to party and alliance members. | **grayed/disabled in rent-a-room**; menu-box label renders truncated as "Open Mog" |
| 7 | Remodel | Change house style. | not entered (avoid changing state) |

Rent-a-room rule (user-confirmed retail behavior): Layout and Open Mog House are
disabled when the MH is not in the character's home nation.

## Storage submenu (15 entries, exact order)

Mog Safe, Mog Safe 2, Storage, Mog Locker, Mog Satchel, Mog Sack, Mog Case,
Mog Wardrobe, Mog Wardrobe 2 … Mog Wardrobe 8.

Help texts (help-bar title truncates long names, e.g. "M.Ward.", "Safe 2"):

- Mog Safe / Mog Safe 2: `Leave items in the care of your moogle.`
- Storage: `Leave items in the extra storage space of your Mog House furniture.`
- Mog Locker: `View content of Mog Locker. Items can only be removed or stored when inside your Mog House.`
- Mog Satchel: `Remove or store items in your Mog Satchel.`
- Mog Sack: `Remove or store items in your Mog Sack.`
- Mog Case: `Remove or store items in your Mog Case.`
- Mog Wardrobe 1–8: `Remove or store items in your Mog Wardrobe.`

## Delivery Box submenu

Two entries: **Receive** (`Receive items.`) / **Send** (`Send items.`).

### Send flow ("Deliveries" panel) — exercised end-to-end (sent 1x Pet Fd. Gamma to alt "Atti")

Panel header: `Deliveries | After specifying recipient, place items in empty slots to send them to recipient's delivery box.`
Layout: 8-slot grid (2×4) with gil amount bar + OK/Cancel; Recipient text field
with OK/Exit; inventory item list (right); Current Gil display (bottom-left);
item description panel for the highlighted item.

Flow and states:
1. Cursor starts at Recipient field → Enter opens text input → type name → Enter (name turns yellow).
2. Down to recipient OK → Enter locks recipient; cursor moves to slot grid.
3. Enter on empty slot → item list activates (`Items | Select an item.`).
4. Pick item → quantity spinner (`No. of Items | Set the number of items.`, `All ◀ 1 / 2 ▶` UI) → Enter.
5. Item appears in slot with count; Recipient label becomes **"Recipient (preparing)"**.
6. Recipient-OK press dispatches: label becomes **"Recipient (sent)"**. No fee charged (gil bar stayed 0 G on Horizon).
7. Esc/Exit closes back to Receive/Send.

Rules observed: **Rare/Ex items are grayed out** in the send list (She-Slime
Shield, Warp Cudgel); stackables prompt for quantity; sent item leaves inventory
immediately (Pet Fd. Gamma 2 → 1).

### Receive flow ("Delivery Box" panel)

Header: `Delivery Box | Select an item from the delivery box.`
Layout: same 8-slot grid; action buttons **Take / Drop / Return**; Current Gil.
(Oldman's box was empty; pickup of the pending Atti mail is a follow-up —
log in as Atti to observe the has-mail state.)

## Change Jobs submenu

Entries: **Main Job** (`Set your main job.`) / **Support Job** (`Set your support job.`).
Main Job opens the job grid: header `Change Jobs | Select the job you want to change to.`;
two-column list, unlocked jobs show `L.<n>`, locked jobs render as `???`,
current main job's level drawn in yellow (Beastmaster L.47).

## Gardening

Opens a furnishing-source picker: header `Mog Safe | Select furnishing from your Mog Safe.`
with submenu **Mog Safe / Mog Safe 2** (choose where the flowerpot furnishing
lives). Not driven further (no flowerpots owned).

## Home-nation Mog House (Atti, Hume THF15, Bastok Markets MH 1F) — observed 2026-07-18

Zone-in chat marker: `=== Area: Mog House 1F ===`. Menu labels/order identical to
the rent-a-room; enabled-state differences:

| Entry | Rent-a-room (Jeuno) | Home nation (Bastok) |
|---|---|---|
| Layout | disabled | **enabled** |
| Open Mog House | disabled | **enabled** |
| Remodel | (not entered) | **disabled/dimmed** — no house restyle on a 75-era server |

- Esc backs out one level; root cursor position retained (re-confirmed).
- **Layout**: camera flips to a top-down overhead view of the room; help
  `Layout | Select furniture to place in your room.` Moogle chat line:
  `Moogle : You can place new furnishings by selecting "Layout," kupo!`
  **Exiting Layout grants/recomputes the Moghancement key item** — chat:
  `Obtained key item: Moghancement: Earth.`
- **Open Mog House**: no confirm dialog — selecting it applies immediately; help
  bar title becomes `Mog House | Open your Mog House to party and alliance members.`
- **Remodel**: help `Remodel | Change house style.`; row renders dimmed.

### Delivery Box receive with pending mail (has-mail state)

Opening Receive with queued mail auto-loads the oldest queued item into slot 1:
grid slot occupied + selected, `Sender:` name under the grid (Oldman), item
description panel for the selected slot, buttons **Take / Drop / Return**,
Current Gil. (An Ashita-side list overlay also showed the remaining queue —
addon UI, not retail.)

Take flow: Enter on the occupied slot → cursor moves to the buttons (Take
default; help `Delivery Box | Take item out of delivery box.`) → Enter → chat:

    You take the Pet Food Gamma biscuit out of delivery slot 1.

— full item log-name, **1-based** slot number. Slot empties; panel stays open
with the header back to `Select an item from the delivery box.`

## Capture index

- moghouse-home-nation-menu.png — home-nation menu (Layout/Open Mog enabled, Remodel dimmed)
- moghouse-deliverybox-hasmail.png — Receive panel with pending mail (Sender: Oldman, Take/Drop/Return)
- moghouse-deliverybox-take.png — post-Take state + "out of delivery slot 1" chat line
- moghouse-layout-open.png — Layout overhead placement view
- moghouse-openmog.png — Open Mog House applied (help-bar title "Mog House")
- moghouse-menu-item0..7.png — main menu, each entry highlighted (help bar per item)
- moghouse-storage-submenu.png — Storage container list (15 entries)
- moghouse-deliverybox-submenu.png / -send.png — Receive/Send submenu + help texts
- moghouse-deliverybox-open.png — Send panel initial state
- moghouse-deliverybox-send-confirm.png — post-dispatch state ("Recipient (sent)")
- moghouse-deliverybox-receive.png — Receive panel (Take/Drop/Return, empty box)
- moghouse-changejobs-submenu.png / moghouse-mainjob-list.png — Change Jobs + job grid
- moghouse-gardening.png / moghouse-gardening-source.png — Gardening source picker
- moghouse-layout.png — main menu with Layout highlighted (disabled state)
