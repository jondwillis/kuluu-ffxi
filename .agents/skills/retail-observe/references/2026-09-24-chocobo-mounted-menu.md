# The chocobo-mounted command menu, 2026-09-24

Screenshot provided by the owner while riding a rented chocobo on HorizonXI
(`2026-09-24-chocobo-mounted-menu.png`, same folder). The character's command
menu — the one that opens on the self target — shows exactly three rows while
mounted:

1. `Chat`
2. `Dig`
3. `Dismount`

It replaces the normal self menu (no Magic / Abilities / Items / Check rows
while mounted). The frame also shows a dig landing in the log
("Obtained: Clump of moko grass. Your wing skill improved to 3!"), so `Dig`
picks up ground items at the chocobo's position.

Corroboration in the retail client analysis: `research/xim/src/jsMain/kotlin/xim/poc/ui/ActionMenu.kt`
defines the menu items `Dig(38, "menu keytops3")` and `Dismount(39, "menu keytops3")`,
and `research/xim/src/jsMain/kotlin/xim/poc/game/UiState.kt getCurrentActions`
appends `ActionMenuItem.Dismount` whenever `playerState.mountedState != null`,
with `isEnterPressed` on that item calling
`GameClient.submitDismountEvent(ActorManager.player())` — a self-targeted
0x01A action.
