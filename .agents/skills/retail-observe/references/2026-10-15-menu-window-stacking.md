# Menu window stacking: every open window stays on screen

Observed 2026-10-15. Scope: clean retail-2026-09. Static binary reading plus two retail screenshots of the Status screen (before and after Enter on Status). No Kuluu runtime was driven. This record states how retail keeps several menu windows on screen at once, where each one sits, and which one takes the keys. Family-specific chains (Status details, merits, Unity, Config pages) only add which window opens which.

## Windows are siblings, not replacements

Selecting a row that opens another window leaves the opener open. Both windows stay on screen until something closes them. Nothing in the DAT window records links a child to a parent; the link exists only in the opener's behavior, and each family's record lists those links.

Windows draw in one list per priority, oldest first, newest last, so the newest window covers older ones where they overlap. Frames are opaque. All Status, Config, merit, Unity and Alter Ego point windows share one priority, so for them "on top" simply means "opened later".

A window that sits exactly on an older window's rectangle hides it completely. Example: the Status selector `statcom2` lands on the same x, y and width as the main menu `menuwind` and is one row taller, so the main menu vanishes while Status is open. It is still open. Esc out of Status uncovers it with its cursor still on Status.

## Where each window sits

Every window sits at its own DAT window record frame: x and y in the 512 x 448 design space, adjusted by the frame's anchor byte (0 none, 1 right edge: x += screen width - 512, 2 bottom edge: y += screen height - 448, 3 both). The width and height also come from the record. Nothing measures the opener, and no window is placed relative to the window that opened it unless its family record says so.

Known code moves that override the record frame:

| window | situation | position |
| --- | --- | --- |
| `persona` | opened by main menu Status | (16, 16) |
| `persona` | its second mode | (16, 48) |
| `joblevel` | opened from Status | (130, 48) |
| `joblevel` | job change mode | (16, 300), and it becomes interactive |
| `level` | opened with Status | its record frame |
| `level` | its mode 1 | record frame, y + 20 |
| `level` | its mode 2 | record frame, y + 52 |
| `levmerit` | Monstrosity | record frame, y + 20 (same window class as `level`) |

Docked windows (registry docking flags) keep the record x and compute y from an anchor window; their own records state the anchor.

## Keys

Exactly one window gets the keys: the newest open window that is not display-only and not closing. Display-only windows (`persona`, `level`, `profile`, `joblevel` in Status, `btlskill`, `mgcskill`, `trdskill`, `masterle`, `levmerit`, `levmeri2`, `meritinf`, `fp_info`, `ut_point`) never get keys, so the window under them keeps navigating. When the window holding the keys closes, the keys fall to the next newest focusable window with no extra step.

Esc goes to the window with the keys. If its registry entry routes Esc to the window's handler, the handler decides what closes. Otherwise the window closes itself. In Status, Esc with a display-only detail showing closes only the detail. With no detail showing, it closes `statcom2`, which closes `persona`, which closes `level`.

## Config

The main menu's page 2 row Config opens `configwi` and page 2 stays open under it. A `configwi` row opens one page window. `configwi` stays visible at its own frame while the page sits at the page's frame and takes the keys. Esc on the page closes the page (by the page's own handler or by itself, per its registry entry). The keys return to `configwi`, and Esc again closes `configwi`.

Some pages open a third window: the key layout window opens next to the page's own rectangle, and the controls page opens its two key-assign windows. All three levels stay visible together.

## Established and not established

Established: siblings stay open; draw order is oldest to newest; frames at their own record rectangles plus anchor; the code moves listed above; the focus and Esc rules; the Status and Config cascades.

Not established: the extra half-width and half-height offset the code-move helper adds when a menu-manager scale field is nonzero (it is zero in the observed input mode, so the listed positions hold there); exact screen size used for anchors when the UI is scaled; the key layout window's offset from its page.

## Provenance

All code locations are RVAs in retail-2026-09 (TDS 0x6A995428, image base 0x10000000).

OpenMenu RVA 0x15E350 (activate 1, exclusive 0 keeps the opener). Close by name RVA 0x15E910, close instance RVA 0x15E790. Priority insert RVA 0x15DB10. Draw pass RVA 0x15F6A0 (priority 0..8, list order, skips closing windows). Focus pick RVA 0x15E7E0, set focus RVA 0x15E850, recomputed in the manager update RVA 0x15FA00. Input to focused window only RVA 0x15DD00; Esc self-close without flag 0x2 at RVA 0x11844D.

Registry RVA 0x3722D8 (0x2C-byte entries; flags at +0x24; flag 0x1 display-only, 0x2 Esc to handler). Frame position and anchor RVA 0x1193F0 (screen W, H at menu manager +0x80/+0x82). MoveTo RVA 0x119110, code-move helper RVA 0x1602F0 (adds W*s/2, H*s/2 with s at manager +0x84/+0x88).

menuwind Status case RVA 0x1404B2 (persona mode 1, OpenMenu persona). persona open RVA 0x146EC0 (mode 1 moves to (16,16), opens level and statcom2), persona close RVA 0x146FD0 (closes level). level open RVA 0x147620 (+20 / +52 from RVA 0x3716A4). joblevel mode setter RVA 0x1482D0. statcom2 select RVA 0x148D00, close RVA 0x148BB0 (closes persona), close-details RVA 0x148AC0. levmerit mode write in statcom2 select at RVA 0x148E3B under the predicate at RVA 0x97D50. statcom2 frame vs menuwind frame from the Status screenshots and the DAT reads banked by the menu epic.

socialme select opens configwi RVA 0x14E8D9. configwi select RVA 0x196770 (jump table RVA 0x1968A4). keylayou opened and moved RVA 0x196DC5, 0x1971D5, 0x19B159. conf6win opens k1assign/k2assign RVA 0x19B2E6, 0x19B2FE.
