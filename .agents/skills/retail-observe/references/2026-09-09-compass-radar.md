# Compass radar evidence, 2026-09-09

This record separates observed retail output, authored DAT data, and unresolved
runtime rules. It establishes the standing compass radar as vanilla HUD behavior;
it does not establish that the upper-right terrain minimap in the user reference
is vanilla. The retail VM was suspended during this investigation, so no fresh
login, camera drive, or distance calibration was performed.

## Primary sources and visible output

Square Enix's [FFXI manual](https://support.na.square-enix.com/document/manual/20/FFXI_manual_vc09_AE5.pdf),
printed page 20, "PLAYING THE GAME", "THE GAME SCREEN", item 3, specifies:

| Entity | Compass dot color |
| --- | --- |
| Other player character | Blue |
| Non-player character | Green |
| Party member | Pink |
| Pet or avatar | Yellow |
| Monster, for eligible jobs | Red |

The downloaded publisher PDF was text-extracted and its complete relevant spread
visually inspected. Local reference: `artifacts/verify/compass-reference/retail-manual-official.pdf`,
SHA-256 `f9d1b8045d88aaf7ae97e9ff8c2043b688644449aefb6bc27899a1dd8eaef13e`.
The rendered spread is `artifacts/verify/compass-reference/official-manual-compass-page20.png`.

Square Enix moderator Qeepel's reply dated 2012-03-31 in
[No mobs show on the compass PS2/PS3](https://forum.square-enix.com/ffxi/archive/index.php/t-22277.html)
confirms that the main **or support** job can enable enemy red dots. The named
jobs are Thief, Beastmaster, Ranger, Ninja, Summoner, and Blue Mage. This is an
official clarification of the manual's unspecified job condition, not an
inference from which monsters happen to appear in one screenshot.

The user-supplied 1920x1080 reference, received 2026-09-09, is locally available
as `codex-clipboard-fdf62e51-6052-4d31-b60b-8d02cb53f07c.png` in the session's
clipboard directory. Its extracted compass crop is
`artifacts/verify/compass-reference/user-compass-crop.png`. It shows an approximately
104x52-pixel elliptical dial, excluding the cardinal glyphs, above the day orb,
clock, coordinates, and chat. The glyph cluster occupies roughly 136x74 pixels.
North is red; the other cardinals are pale. Green dots are visible. These are
approximate visual bounds in the supplied image, not retail layout constants.
The user's stated camera-dependent rotation is task evidence; a still image by
itself does not establish the rotation algorithm.

A pre-existing retail/HorizonXI capture,
`artifacts/retail/20260719-143905.png`, independently shows the same arrangement,
upright cardinal glyphs, blue clustered dots, and green dots. Its dial occupies
approximately 74x35 source-image pixels, excluding glyphs. The crop
`artifacts/verify/compass-reference/retail-20260719-compass.png` comes from source
rectangle `(780,1490)-(920,1577)` and is enlarged fivefold for inspection. It has
S/W above the dial and E/N below. The different absolute size from the user
reference does not establish a different game rule: HUD scale/configuration was
not recorded for either capture. Both support an approximately 2:1 dial aspect.
The older session runs through Ashita/HorizonXI; injected UI modifications have
not been exhaustively excluded.

## Installed DAT evidence

With `FFXI_DAT_OVERLAYS=''`, the existing `ffxi-dat` resolver maps file ID 39542 to
`vendor/game-files/SquareEnix/FINAL FANTASY XI/ROM/119/51.DAT`. Its SHA-256 is
`3569f59f638b624c2c0bcc0f3338eecb725b70003296aa9dd93f591c17feff61`.
The normal environment instead resolves an xiview overlay at
`vendor/game-files/polplugins/DATs/xiview/ROM/119/51.DAT`; both have the component
layout described below. Base-data metadata and exports are local in
`artifacts/verify/compass-reference/dat-metadata.txt` and
`artifacts/verify/compass-reference/base-compass-sprite-strip.png`.

The `menu    compass ` UI element group has six entries:

| Index | Visible art | Authored component data |
| --- | --- | --- |
| 0 | Dial | Four mirrored quadrants; each samples the same 32x32 texture region. Positions span -128 to +128 across the composed element. Flip modes are 0, 1, 2, 3. |
| 1 | E | 16x16; vertex color bytes `[96,96,96,127]`. |
| 2 | W | 16x16; vertex color bytes `[96,96,96,127]`. |
| 3 | S | 16x16; vertex color bytes `[96,96,96,127]`. |
| 4 | N | 16x16; vertex color bytes `[127,64,64,127]`. |
| 5 | Red dot | 6x6 texture crop, centered component positions from -3 to +3. |

Cardinal texture pixels themselves are grey; north's red appearance is supplied
by its authored component tint. Entry 0 uses `[127,127,127,127]` vertex colors.
The byte values are reported as parsed, without asserting the final retail GPU
color equation. `ffxi-dat/src/ui_element.rs::ui_sprite` extracts only the first
component and omits vertex tint. Consequently, uploading index 0 as one ordinary
sprite gives one quadrant, not a complete dial; uploading index 4 without its
tint gives a grey N. A consumer must account for those component properties.
Raw assets and screenshots remain local and must not enter git.

## Coordinate and camera interpretation

`kuluu-render/src/scene.rs::ffxi_to_bevy` maps wire position to
`(x,-wire.z,-wire.y)`. `minimap::MinimapAabb::world_to_uv` maps renderer +X right
and +Z down on the map. The matching renderer cardinal basis is therefore north
-Z, east +X, south +Z, west -X. A camera located on +Z and looking toward the
origin faces north in this basis; camera location and viewing direction must
not be conflated.

The community reconstruction
`research/xim/src/jsMain/kotlin/xim/poc/ui/Compass.kt::drawCompass` uses the active
camera's horizontal view vector, transforms it through the current area when
needed, rotates the dial, and positions the separate cardinal elements. It uses
one-quarter scale for the 256-unit authored dial, yielding a 64-unit composed
dial before any outer UI transform. `MapDrawer.kt::getMapPosition` maps its
unflipped retail +Z to map up. These are useful corroborating implementation
references, not a disassembly proof of the precise retail projection.
`research/XIClient` contains compass menu names and a placeholder but no
implemented compass draw routine was found.

## Unresolved rules

The exact distance scale, outer-dot cutoff, edge behavior, and Ninja-specific
size/range have not been measured from retail or traced through FFXiMain.dll.
A [BG Wiki radar entry](https://www.bg-wiki.com/ffxi/Radar) reports a 20-yalm outer
ring and 25 for Ninja; those remain community leads, not verified numerical
constants from this investigation. Its linked
[official 2011-02-18 Q&A](https://www.playonline.com/pcd/topics/ff11us/detail/6258/detail.html)
was retrieved and inspected: it addresses making the compass visible during
combat in the next major update and contains no range or Ninja measurements.
The page is preserved locally as
`artifacts/verify/compass-reference/official-faq-20110218.html`. A 50-yalm outer
ring has no supporting source here.

The documented entity classes and enemy main/support-job allowlist are grounded.
More detailed eligibility remains unverified: invisible or despawning entities,
dead actors, alliance members, trusts, object NPCs, vertical separation, and
whether selection changes any dot rendering. Likewise, the exact dial tilt
computation, response to camera pitch, menu occlusion, and hidden-state animation
were not isolated. The visible references support an ellipse and upright
cardinals; a fixed 0.5 vertical scale is an implementation approximation until
a controlled camera comparison or binary trace settles it. Passing a build or
rendering a plausible dial does not close these parity questions.

## Opacity follow-up

The user's follow-up identified excessive transparency. A direct comparison of
the base and xiview DATs found byte-identical compass component metadata, all
six sprite crops, and the complete `menu compass`, `menu news`, and `menu marker`
texture chunks. The containers differ elsewhere. Evidence and hashes are in
`artifacts/verify/compass-opacity/comparison.json`.

The dial and letters use DXT3: decoded alpha is the raw nibble expanded by 17,
with maxima of 102 and 136 respectively. `UIManager::InitDraw` in
`research/XIClient/src/XIClient/source/UI/UIManager.cpp` sets alpha modulation
to MODULATE2X. `UIShapeQuad::ParseFromResource` supplies the adjusted vertex
alpha; `Handle_0_1_1` uses ordinary source-alpha blending for these components.
This is community reconstruction evidence, without a fresh binary trace.

Kuluu omitted that factor of two for composed DXT3 UI art. The correction bakes
twice the texture-times-vertex alpha into the uploaded image, saturating after
the product; the ImageNode then uses unit alpha. Transparent texels stay clear,
RGB stays unchanged, and palette decoding keeps its existing alpha conversion.
