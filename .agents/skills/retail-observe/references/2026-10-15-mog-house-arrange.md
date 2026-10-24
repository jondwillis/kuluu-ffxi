# Mog House furniture drawing and the arrange (layout) editor

Observed 2026-10-15. Scope: clean retail-2026-09, static reading of FFXiMain.dll only; no client or
server was run. Part A says how every placed piece is drawn (model, position, rotation, visibility).
Part B says how the arrange editor behaves (keys, selection, the `rmlo*` windows, moving, packets,
sounds, messages). It extends the ingest law already in kuluu (`myroom_furniture.rs`) and the mode
law in `myroom_layout.rs`. Text strings are DAT content and are named by id only.

Conventions: positions are FFXI world coordinates (y grows downward, so "up" is negative y), in
world units. "Cell" is one floor grid square. `col`, `level`, `row`, `rot` are the furniture Attr
bytes extdata[6], extdata[7], extdata[8], extdata[9] (signed), which LSB names X, Z, Y, Rotation
and reads back through `getCol`/`getLevel`/`getRow`. "Floor" is the current Mog House floor
(0 = 1F, 1 = 2F; already derived in kuluu from s2c 0x00A MyroomSubMapNumber). Colours are given as
(R, G, B, A) bytes where 0x80 means 1.0.

## Part A: drawing placed furniture

### A1. Furniture property table (the DAT the client reads)

Every piece is described by an 8-byte record in DAT file id 32922 (0x809A): the payload of the
first chunk of kind 0x59 in that file (the bytes after the 16-byte chunk header). The record for
item id `i` is record number `i` when `i < 3584 (0xE00)`, else `i - 3072 (0xC00)`.

| Bytes | Meaning |
|---|---|
| 0 | flags: bit 0 wall-hung; bit 1 surface (other pieces can stand on it); bit 2 stackable (rises onto a surface when moved over one); bit 3 flowerpot (carries a plant model) |
| 1 | not read by any furniture code found |
| 2-3 | u16 model number |
| 4 | footprint width in cells along x at rot 0 |
| 5 | footprint depth in cells along z at rot 0 |
| 6-7 | u16 height in hundredths of a world unit (surface top height; level of a piece on it is height / 10, truncated) |

The same DAT also holds the plant table (first chunk of kind 0x5B, A5) and three sound chunks of
kind 0x3D named `8058`, `8059`, `8060` (B9).

### A2. Model number to model file

For pieces whose item kind is 10 or 12 (14 is a mannequin, A6):

- model number 264..299: no model file. The client creates a character entity that looks exactly
  like an NPC sent in s2c 0x00E with the standard look kind (LSB look.size 0, MODEL_STANDARD) and
  model id n: every other look word 0, empty name. In kuluu terms that is
  `EntityLook::Standard { modelid: n }`, so the NPC model path applies. It is placed and turned
  like any other piece (A3). Its motions as a client-made object were not traced.
- otherwise file id = n + 32927 for n < 256; n + 51183 for 256 <= n < 512; no model for
  512 <= n < 3584; n + 59227 for 3584 <= n < 4096; 32927 for n >= 4096. (Hex: 0x809F + n,
  0xC7EF + n, 0xE75B + n.) The file id resolves through the install's FTABLE/VTABLE as usual.

The static model is given a box of (width × 0.6, height × 0.01, depth × 0.6) world units at rot 0
(used by camera collision; not traced here).

**What the client takes from a static model file.** Not the character pipeline: each piece is one
static-model object, the same kind the client uses for a transport (ship) model file (kuluu
`transport_file_id`, model + 31004). From the file it uses:

1. Exactly one mesh, a kind-0x2E (MMB) chunk, picked as (a) the first kind-0x2E chunk named
   `fune`, else (b) the first named `_fun`, else (c) the first kind-0x2E chunk of any name. "First"
   is file order with nested directories searched. Any other MMB chunks in the file are not drawn.
   The chosen chunk is parsed exactly like a zone MMB (same header de-obfuscation, block and part
   layout, 36-byte vertices of position, normal, colour, uv; strip or list by MMB type), so that
   one mesh can still hold several parts, each naming its own texture.
2. Textures by name. Each part's 16-character texture name is matched first against the file's own
   kind-0x20 (IMG) chunks (the 16 bytes after the IMG payload's first byte), then against textures
   already loaded from any file with the same 16 characters, then against any loaded texture whose
   last 8 characters match. A part with no match draws with no texture bound. A file can therefore
   carry several IMG chunks, and a part may also use a texture another loaded file provides.
3. Schedulers, under rule (c) only: on load the client starts on the piece every kind-7 chunk of the
   file that passes a test on its command types (test not traced), and once the file is resident it
   also starts the kind-7 chunk named `init` if there is one (one more per-object condition not
   traced). Under (a) and (b) no scheduler is started.
4. Under (a) and (b) the drawn rotation eases toward the piece's rotation: each update, once per
   frame tick (tick count rounded up, at least once), it moves 1/8 of the remaining difference
   (yaw difference wrapped to ±π). Under (c) it follows at once. Position never eases. Whether any furniture file
   names its MMB `fune` or `_fun` could not be checked (no DATs here).

Draw state, one instance with its own world transform (A3, A4), is the zone MMB pass's state:

- fixed-function lighting on, with the same environment light(s), ambient and fog the zone pass
  uses; texture stage colour = texture × lit vertex colour × 2, alpha = texture alpha × vertex
  alpha × 4 (as kuluu's zone MMB path already models);
- per-part flags, the u16 after the texture name (kuluu `MmbRenderState`): 0x8000 turns on alpha
  blending (source alpha, inverse source alpha) for that part, otherwise blending is off; 0x2000
  draws both faces, otherwise back faces are culled with the zone winding;
- alpha test for the whole piece when its mesh came from rule (b), or from rule (c) with a chunk
  name starting `_`: a pixel is kept when its final alpha is above 0x60/0xFF, the same function and
  reference the zone pass sets. Otherwise no alpha test;
- inside a Mog House (so for every drawn piece) one more scene light is switched on: the light the
  client holds under the 4-character name `l_01`. If none exists that light stays off. Where `l_01`
  is defined was not traced.

### A3. Floor pieces: position and rotation

Per floor: grid origin `(ox, oz)` and bounds (cells `0 <= col < 20`, `0 <= row < R`):

| Floor | ox | oz | R |
|---|---|---|---|
| 1F | 10 | 12 | 24 |
| 2F | 10 | 13 | 26 |

With `w`, `d` the footprint after rotation (swap width/depth when `rot` is odd):

- x = (col − ox) × 0.6 + w × 0.3
- z = (row − oz) × 0.6 + d × 0.3
- y = −level × 0.1 (a level-10 piece sits 1.0 unit above the floor)
- rotation = Euler (0, rot × π/2, 0) on the model (yaw only, 90° steps, rot 0..3), the same
  rotation channel any placed model instance uses.

So the model origin is the centre of its footprint; `(col, row)` is the footprint's minimum corner.

### A4. Wall-hung pieces (flag bit 0)

`col` is a wall-slot index; `level` and `row` are 0. The position is the slot's point (no footprint
offset); the drawn rotation is still `rot` (extdata[9]).

1F has 5 slots (y = −3.0, i.e. 3 units up). Slot points (x, z): 0 = (6.5, 4.55), 1 = (6.5, −4.8),
2 = (−6.5, −4.8), 3 = (−6.5, 0.0), 4 = (−6.5, 4.55); when s2c 0x00A MyroomMapNumber is 289 (0x121,
LSB's San d'Oria home house) every x is ±6.2325 instead of ±6.5. Slot rotation: 1, 1, 3, 3, 3.

2F has 92 slots = 23 wall columns × 4 heights (h 0..3 → y −5.6, −3.2, −2.5, −1.8). Slot =
base + 4 × column + h:

| Wall | base | columns | point | slot rotation |
|---|---|---|---|---|
| West | 0 | 9: z = −6.2, −5.15, −4.1, −1.0, 0.0, 1.0, 4.1, 5.15, 6.2 | x = −6.2 | 3 |
| North | 36 | 3: x = −4.0, 0.0, 4.0 | z = 8.0 | 0 |
| East | 48 | 9: same z list as West | x = 6.2 | 1 |
| South | 84 | 2: x = −4.0, 4.0 | z = −8.0 | 2 |

### A5. Flowerpots (flag bit 3)

A pot with extdata[0] (LSB FlowerPot Step) > 0 carries a second static model: model number =
plant table value for row `extdata[4] − 1` (LSB FlowerPot Kind; row clamped at 0) and column
`extdata[0]`; value 999 means no plant model. Plant table layout (payload of the kind-0x5B chunk):
u32 row offset for row r at payload offset 4 + 4r; the value for column c is the u32 at payload
offset (row offset + 4 + 4c). The model file comes from the A2 formula. The plant is drawn at the
pot's position raised 0.3 (y − 0.3) with the pot's rotation, and is rebuilt whenever extdata[0]
changes.

### A6. Mannequins (item kind 14)

Drawn as a character, not a property-table model: race = extdata[18], pose = extdata[19] (LSB
Mannequin Race, Pose). Idle motion family is the 4-character name `mq0?` .. `mq7?` with digit
`pose & 7` and variant `pose >> 3`; pose 8 instead takes the digit from a byte of the item info of
one dressed item in the look record (default 6; probably the main weapon's type) with variant 1.
Equipment comes from a per-slot look record the client keeps for the shelf entry (not traced; LSB
fills mannequins through s2c 0x026). While the mannequin dress window (`inspect`) has it selected
the mannequin turns 3° per client update.

### A7. Visibility

Pieces are created for every qualifying record but are drawn only when the house is arrangeable:
floor 1 (2F), or floor 0 with s2c 0x00A MyroomMapNumber 289, 290 or 291 (LSB's home-nation houses)
while LoginState puts the character in a Mog House. In any other Mog House (rent-a-room) the
models (and plant models) are hidden and the editor does nothing (B1).

A separate rule shows room NPC entities whose model id is 1847 only while item 286, 287 or 426
(LSB: Nanaa Mihgo statue, statue II, Orchestrion) is placed on either floor, and hides them in a
non-arrangeable house.

### A8. Ingest refinements

The shelf scan runs every client update over containers 1 then 9 (not in zone 280, the Mog
Garden). It creates a piece only when none exists for that seat; an existing piece keeps its
client-side pose (a later record for the same seat re-poses it only if its mannequin look changed).
A piece whose record later shows the installed bit clear, after having been seen installed, is
destroyed. Records installed on the other floor are not created.

## Part B: the arrange editor

### B1. Entering and leaving

Entering (mode 1, from `/layout` or the Moogle menu's Layout row) does nothing at all in a
non-arrangeable house (A7). Otherwise, beyond the window closes and input context already in
`myroom_layout.rs`, it: sets every piece's tint to (0x80, 0x80, 0x80, 0x80); puts the help bar to
title GetText(13, 4) and help GetText(14, 14); selects the requested or first piece (first = lowest
seat, container 1 slots then container 9 slots); opens `rmlo1` only when nothing is placed on this
floor.

`/layout` refusals in order, after the generic busy checks: client message 188 while a state value
delivered by s2c 0x067/0x068 is nonzero (field not traced); client message 5 when the house is not
arrangeable (A7); error buzzer when already arranging or the door is open; client message 6 when
not in a Mog House. On success it prints system message 54 (sysmes table, kuluu SYS_MES_FILE_ID)
and enters.

Leaving (mode 0: Esc in the browse state, or `rmlo1` item 3) clears the selection, resets tints,
clears title and help, restores input context 0xFFFF, reopens `myroom` if it is not open, and sends
the all-zero c2s 0x0FA. The client also forces mode 0 when the player becomes busy (event) while
arranging.

On the first update after a mode change the client stores the camera field-of-view parameter and
sets it to 250 for layout (the normal camera starts at 350; FOV = 2·atan(192 / value), so about
75° instead of 57.5°), and restores the stored value on leaving. On both entering and leaving it
sets the local player's position to (0, 0, 0) on 1F or (0, 0, −9) on 2F and toggles a display flag
on the player model (meaning not traced).

### B2. Browse state (no piece being moved, no window focused)

The editor polls these actions (input context 0x42); default keys (none with Ctrl/Alt held):

| Action | Default key | Effect |
|---|---|---|
| Select left/right/up/down | arrow keys, auto-repeat | move the selection (B3); cursor sound on every press |
| Open piece menu | Enter, keypad Enter, single press | confirm sound, then B4 |
| Leave | Esc, single press | cancel sound, mode 0 (B1) |
| Cycle view | keypad 5, single press, only with a selection | view 0 → 1 → 2 → 0 (B5) |

(Each action also has a Ctrl/Alt chord binding gated by a client keyboard-mode flag and a gamepad
binding; not mapped here.)

Highlight: the selected piece pulses: its tint is (g, g, g, a) with g starting at 0x80 and moving
8 × (frame ticks) per update, falling to below 0x20 then restarting at 0x40 rising, capped at 0xFF
then falling again; a = 0x40 (0x80 for mannequins). Pieces standing on it pulse with a = 0x40. The
pulse restarts on every selection change (resetting all tints first). An animated cursor icon (UI
sprite group 6, frame every 3 ticks) is drawn at the selected piece's projected screen point
(coordinate 100 used when a projection is negative).

### B3. Selection navigation

A press gives (dx, dz): Right (1, 0), Left (−1, 0), Up (0, 1), Down (0, −1); in view 2 both signs
flip. Let P be the selected piece's drawn (x, z); a wall piece's point is pushed by its height
h = −y toward its rotation (rot 0: z + h, 1: z − h, 2: x + h, 3: x − h); wall candidates get the same
push. For each other drawn piece Q, Δ = Q − P:

1. If the selected piece has level 0 and |Δx| + |Δz| < 0.1, Q is chosen at once.
2. Q qualifies when it lies past the selected piece in the pressed direction: Δx·dx > t and
   |Δx| ≥ |Δz| for horizontal presses, Δz·dz > t and |Δz| ≥ |Δx| for vertical ones (both when two
   arrows are held). t = 0 if pieces stand on the selected piece or it is wall-hung, else
   width × 0.3 (horizontal) or depth × 0.3 (vertical) of its rotated footprint.
3. Take the qualifier with the smallest Δx² + Δz²; if the best level-0 qualifier is within 0.1 of
   that squared distance, take it instead. No qualifier: selection unchanged.

### B4. The piece menus and pickers

Enter with a selection opens `rmlo2` (or `rmlo4` for a mannequin). With no selection it selects the
first piece silently, or opens `rmlo1` when nothing is placed. All three are DAT window records;
their item captions come from the DAT. While one is up the input context is 0x3E (furniture actions
off); Esc closes it without calling the handler and context returns to 0x42.

| Window | Item id | Action | Disabled (grey) when |
|---|---|---|---|
| rmlo1 | 1 | place furniture: open shelf picker | |
| rmlo1 | 2 | put away all | the Storage container (2) list holds more than one entry |
| rmlo1 | 3 | finish: leave arrange (B1) | |
| rmlo2 | 1 | move (B6) | |
| rmlo2 | 2 | put away this piece | Storage free slots < Storage value of this piece plus everything on it |
| rmlo2 | 3 | place furniture | |
| rmlo2 | 4 | put away all | as rmlo1 item 2 |
| rmlo4 | 1 | dress: open `inspect` for the mannequin | |
| rmlo4 | 2 | move | |
| rmlo4 | 3 | put away this mannequin (window stays open) | as rmlo2 item 2 |
| rmlo4 | 4 | place furniture (rmlo4 closes at once) | |
| rmlo4 | 5 | put away all | as rmlo1 item 2 |

Any other item id just closes the window. "Storage value" is the item's furnishing storage-slot
count (POLUtils StorageSlots, LSB item_furnishing.storage); "free slots" is the client's size word
for the Storage container (LSB sends container sizes in s2c 0x01C) minus its list count.

Put away: if the piece or anything on it has extdata[0] > 0 (a planted pot, LSB Step), a yes/no
dialog (`comyn`) with GetText(12, 53) asks first; Yes or no contents → the piece and everything on
it are flagged for c2s 0x0FB. Put away all: yes/no GetText(14, 615); Yes → if any placed piece has
extdata[0] > 0, the GetText(12, 53) dialog; Yes → every piece on both shelves is flagged and the
occupancy grid is cleared.

Shelf picker `bankmenu`: item 1 = container 1, item 2 = container 9 (grey while a client flag
delivered by s2c 0x067/0x068 is clear; kuluu's `mh_2f_unlocked` is the likely equivalent, not
verified). Choosing opens the item window `inventor` on that shelf in furniture mode and closes
the `rmlo*` window; cancelling returns to it. Furniture mode lists kinds 10, 12 (flowerpots only on
1F) and 14; rows placed on this floor are tinted (0x50, 0x90, 0x50, 0x80), unplaced rows normal,
rows placed on the other floor and every other kind are left out. Picking: a flowerpot when 10 pots
are already placed → system message 98, a mannequin when 8 are placed → system message 99, nothing
else happens. Otherwise the piece starts moving (B6) and the confirm sound plays; Esc closes the
list back to browse.

A new piece starts unplaced: layer = current floor, rot 0; a wall piece at slot 0 with that slot's
rotation; a floor piece at (col, row) = (9, 12), or at the selected floor piece's (col, row, level)
when one is selected, then stepped by (−⌊w/2⌋, −⌊d/2⌋) with the B6 step rule.

### B5. Camera

Every update in layout mode: focus target = selected piece's drawn (x, z) clamped to |x| ≤ 4.0 and
|z| ≤ 5.0 (1F) or 6.0 (2F); focus y = −0.3 in view 0, else piece y − 0.3 (no selection: (0, −0.3,
0)). Eye target: view 0 = (fx, −5.0, fz − 0.1) (top-down); view 1 = (0.7·fx, −4.5, max(fz − 4.0,
−5.2)); view 2 = (0.7·fx, −4.5, min(fz + 4.0, 5.2)). Eye and focus each move 0.3 of the way to their
targets per update; in view 0 the eye is then put at distance 7.0 (1F) or 9.0 (2F) from the focus
along the eye-focus line. Entering view 0 (and entering layout while in view 0) sets a draw flag on
the zone's model entries tagged 999 to 0; views 1, 2 and leaving set it to 2 (identity of those
entries, probably the ceiling, not traced). The view survives leaving and re-entering in the same
zone.

### B6. Moving a piece

Starting a move unregisters the piece and everything on it from the occupancy grid, tints every
piece (0x60, 0x60, 0x60, 0x80) and pulses the moving one. Input context stays 0x42:

| Action | Default key | Effect |
|---|---|---|
| Step | arrow keys, auto-repeat | one cell (or wall slot); step sound `8058` when it moved |
| Rotate | keypad −, single press | rot − 1 (mod 4); sound `8059` (floor pieces only) |
| Place | Enter, keypad Enter | valid spot: register, flag for c2s 0x0FA, sound `8060`, back to browse with normal tints; invalid: error buzzer, still moving |
| Cancel | Esc | placed piece: rebuild it and everything on it from their Attr bytes, back to browse; new piece: discarded on the next update (other pieces stay dimmed until the next selection change); cancel sound |

A second rotate action (rot + 1) exists with no default key or gamepad binding.

Direction mapping (floor pieces): Up = row + 1, Right = col + 1, Down = row − 1, Left = col − 1.
With `/layoutctrl type1` (config option 0xC1 stored as 0) all four are reversed whenever the
camera eye's z is ≥ the focus z (view 2); `type2` (stored 1) never reverses.

Step rule for a floor piece by (Δcol, Δrow): clamp the target so the footprint stays inside the
floor bounds (0 ≤ col ≤ 20 − w, 0 ≤ row ≤ R − d). If the clamped target equals the current anchor,
nothing happens. Otherwise everything standing on the piece moves by the same delta (level kept),
the piece's level becomes 0, and a stackable piece (flag bit 2) takes level = height / 10 of the
last footprint cell (scanning col then row) that has a surface under it. Moving ignores overlaps;
validity is only checked by Place.

Wall pieces step between slots by table: 1F slot → (Up, Right, Down, Left): 0 → (0, 0, 1, 4),
1 → (0, 1, 1, 2), 2 → (3, 1, 2, 2), 3 → (4, 0, 2, 3), 4 → (4, 0, 3, 4). 2F, with column c and
height h on each wall (A4): West: Up c+1 (from c 8: North c 0), Right h+1 (from h 3: East same c,
h 3), Down c−1 (from c 0: South c 0), Left h−1 (stay at h 0). East: Up c+1 (from c 8: North c 2),
Right h−1 (stay), Down c−1 (from c 0: South c 1), Left h+1 (from h 3: West same c, h 3). North:
Up h−1 (stay), Right c+1 (from c 2: East c 8), Down h+1 (from h 3: South c 0 h 3 for c 0, stay
for c 1, South c 1 h 3 for c 2), Left c−1 (from c 0: West c 8). South: Up h+1 (from h 3: North
c 0 h 3 for c 0, North c 2 h 3 for c 1), Right c+1 (from c 1: East c 0), Down h−1 (stay), Left c−1
(from c 0: West c 0). Unlisted moves keep the same height. Every wall step sets rot to the new
slot's rotation and plays `8058`. Rotate on a wall piece changes the stored rot without moving
anything and plays nothing.

Rotation of a floor piece keeps its anchor (col, row) and swaps the footprint; then a (0, 0) step
re-clamps it into bounds, and a stackable piece re-levels over the new footprint. Each piece
standing on it gets rot − 1 and anchor = parent anchor + (W' − Δrow − d, Δcol), where Δ is its old
offset from the parent anchor, W' the parent's new width and d the child's depth before its own
rotation.

Overlay while moving, drawn every update as flat quads (cell from (col − ox)·0.6 to +0.6, same
for z): every floor-free cell (B7) in (0x60, 0x60, 0xFF, 0x80) at 0.04 above the floor; for a
stackable piece, every free surface top in (0x60, 0xFF, 0x60, 0x80) at 0.06 above that surface;
the piece's footprint cells in (0xFF, 0x80, 0x80, 0x80) when Place would succeed (plus a second quad
inset 0.05) or (0x60, 0x60, 0x60, 0x80) when it would fail, at 0.06 above each cell's surface
height. A wall piece instead shows every slot on the floor as a rectangle 0.2 off the wall
(1F 3.3 wide × 4.0 tall, 2F 1.0 × 0.6) in (0x60, 0x60, 0xFF, 0x80), its current slot in
(0xFF, 0x80, 0x80, 0x80) plus a 0.9-scaled copy.

### B7. Occupancy rules (Place validity)

Cells: free floor; under a surface piece (floor taken, top free until something stands there; the
cell keeps the surface's height); under any other floor piece (nothing free); blocked: on 1F the
rectangle col 6..13 × row 0..17 (matches LSB `isValidFloorCell`).

- level 0 piece: every footprint cell inside the grid and floor-free.
- level > 0 piece: every footprint cell a free surface top, all of the same surface piece.
- wall piece: its slot empty.

Registering a piece takes its cells or slot. After every registration the client rebuilds "what
stands on what" from the cells (a raised piece belongs to the surface piece under it, at most 32
per surface); moving, rotating and putting away a surface carry those pieces.

### B8. Packets

- Place: c2s 0x0FA for the piece and for every piece standing on it, each with its own item id,
  slot, container, layer (LSB MyroomFloorFlg), col (LSB x), level (LSB y), row (LSB z) and rot
  (LSB v), sent from the next per-piece update; order is seat order (container 1 slots, then 9).
  No packet is sent per step or per rotation.
- Put away: c2s 0x0FB (item id, slot, container) for the piece and everything on it, same timing.
- Leave: the all-zero c2s 0x0FA (LSB treats item 0 as "done", Moghancement update).
- The client never waits for s2c 0x0FA (stub). A put-away piece disappears when its s2c 0x020 shows
  the installed bit clear. After each such removal the client finds the drawn piece nearest in
  (x, z) to the selected piece's position (the removed piece's, if it was the selection; the room
  origin if nothing is selected): that piece becomes the selection unless it already is, in which
  case the selection is cleared.

### B9. Sounds and messages

| When | Signal |
|---|---|
| Browse: direction press | menu cursor sound |
| Browse: Enter | menu confirm sound |
| Browse: Esc | menu cancel sound |
| Furniture list: piece picked | menu confirm sound |
| Moving: step that moved | sound chunk `8058` from DAT 32922 |
| Moving: floor-piece rotation | `8059` |
| Moving: Place accepted | `8060` |
| Moving: Place refused | error buzzer |
| Moving: Esc | menu cancel sound |
| `/layout` | B1 refusals; system message 54 on success |
| List limits | system messages 98 (flowerpots), 99 (mannequins) |
| Put away dialogs | GetText(12, 53); put away all GetText(14, 615) |

## Established vs not established

Established: the property-table source and record layout; model-number → file-id formula and its
264..299 entity exception (standard NPC look, model id n); what a static model file contributes
(one MMB by the A2 rules, textures by name, schedulers only under rule (c)) and its zone-pass draw
state plus the `l_01` light; footprint-centred placement with 0.6-unit cells, 0.1-unit levels and
90° yaw; per-floor origins and bounds; all 1F and 2F wall-slot points, rotations and step tables;
flowerpot plant lookup; arrangeability (2F or MyroomMapNumber 289..291) gating both drawing and the
editor; browse/move key actions, auto-repeat vs single press, step/rotate/clamp/level rules, the
occupancy rules and overlay; the three `rmlo*` item maps and disable predicates; picker and list
behaviour; packet timing (confirm and put away, never per step); sounds and message ids above.

Correction to kuluu's `myroom_layout.rs`: the gate it calls `manager_busy` (message 5) is not a
pending-request test. It is true exactly when the current house is not arrangeable (A7), and the
same predicate makes mode 1 a no-op and hides furniture.

Not established: the mannequin equipment look source and the pose-8 weapon-type field; the
motions of the 264..299 entity; the static-model scheduler test and the `init` condition; where the
`l_01` light is defined; whether furniture files name their MMB `fune` or `_fun`; the exact meaning of the Storage list count in the put-away-all
disable test (read as "Storage not empty", unverified); the item-DAT byte that holds the storage
value (taken from POLUtils, the client loader was not traced); whether the piece selected on entry
pulses before the first selection change (its pulse counter is not set on entry); the predicate
behind `/layout` message 188; the zone entries tagged 999; the player display flag and the
player-position reset; mouse picking in arrange mode (render objects carry a 1/2 state byte that may
be it); gamepad buttons; the rmlo4 dress window. Retail strings (window captions, dialogs,
messages) were not available.

## Provenance

All code locations are RVAs in retail-2026-09 (image base VA 0x10000000, unpacked, raw offset ==
RVA). Static capstone reading only.

Property table: loader RVA 0x1D3120 loads file id 0x809A, chunk kinds 0x59 (manager +0xC) and 0x5B
(+0x10) via chunk lookup RVA 0x72290(&out, kind, name); generic node alloc RVA 0x7183A sizes nodes
payload + 0x30, so node + 0x30 is the payload. Record lookup RVA 0x1D31E0 (`id >= 0xE00 → id −
0xC00`, ×8). Object ctor RVA 0x1CD3B0 stores the record at object +0xF0; getters RVA 0x1CEAF0 /
0x1CEB40 (width/depth by rot, bytes 4/5), 0x1CEBD0 (u16 6), flags RVA 0x1CEBE0 / 0x1CEBF0 /
0x1CEC00 / 0x1CEC10 (bits 0..3). Model path in ctor: 0x107 < n < 0x12C → RVA 0x1CD5FE (entity via
RVA 0xAB2F0); else RVA 0x1CD6B3 → file-id formula RVA 0x1CCE00 → file load RVA 0x731B0 →
instance RVA 0xC3E40; box +0x764..+0x76C. Mannequin path RVA 0x1CD4AE..0x1CD5F9 (race/pose
getters RVA 0xFE990 / 0xFE9C0 read extdata +0x12/+0x13; motion table .data 0x37E028).

Static model object (A2): RVA 0xC3E40 → alloc 0x794 at RVA 0xC3E00 → ctor RVA 0xC3EC0, which runs
the base static-model ctor RVA 0xC41B0 (vtable .rdata 0x330AA0) then sets the furniture vtable
.rdata 0x3306A0 and registers a 0x2AC-byte entity named "Furniture" (.data 0x35D684) with entity
kind byte +0xEE = 3 (the value the s2c 0x00E door case also writes). The same base class serves
transports: ctor RVA 0xC41F0 → RVA 0xC43C0 (file = selector + 0x791C, the routine kuluu's
vehicle.rs cites) or RVA 0xC4380 (file via RVA 0xC51C0(2, model)). Setup RVA 0xC4410(file handle):
RVA 0x72290(&out, 0x2E, 'fune'); else 'fune' miss → RVA 0x72290(&out, 0x2E, '_fun') (alpha flag
+0x770 = 1); both named hits set +0x774 = 4 (first four draws skipped, RVA 0xC4CAF) and +0x778 = 1.
Else (root kind 1) RVA 0x72240 (topmost ancestor, i.e. the self-parented file root) →
RVA 0x72290(&out, 0x2E, 0), then the same from the root; '_' test on the node name byte +0x20 at
RVA 0xC4687; then schedulers RVA 0xC46B0. Lookup semantics: RVA 0x722C0 walks file order through
the next link +8, enters sub-directories (kind 1, end node +0x3C, visit stamp +0x38), and on a
miss widens through the parent link +0x28 until the self-parented root (file roots keyed by file
id +0x34, RVA 0x72FD0); RVA 0x72FB0 just dereferences the handle, so +0x5C4 holds the MMB node.
Schedulers: iterator RVA 0x742B0(root, 7, 0, −1) / RVA 0x745C0; start when RVA 0x56E50 and not
RVA 0x56E80 (command type 1 in the list at +0x78 or 0x52 in the list at +0x74, plus 0xA2 in +0x74,
scanned by RVA 0x56E10 / 0x56DD0); start RVA 0x56C70. `init`: update branch RVA 0xC4A2D..0xC4A7C
(word +0xB2 == 0) calls virtual RVA 0x84C30 (kind-7 lookup by name RVA 0x72430, start
RVA 0x56C70). Update RVA 0xC47F0: position +0x734 ← entity +0xD4 every update; named path eases
rotation +0x744 toward entity +0xE4 by 0.125 per tick (tick count RVA 0x14CF0 = max(1.0, frame
factor)), wrapping with .rdata 0x329D28 / 0x329D2C / 0x329D30 (±3.1415, 6.283); fallback copies at
RVA 0xC4A83. Draw RVA 0xC4C20 builds a 0x16C-byte record at +0x5C8 (MMB object from node +0x30,
word +0xD4 of the record zeroed) and calls RVA 0x1844C0(record, alpha flag) on the zone renderer
(.data 0x631F24); RVA 0x1844C0 has no other caller.

MMB and textures: parser RVA 0x16FCD0 (the routine kuluu's mmb.rs cites for zone MMBs): FVF 0x152
(XYZ, NORMAL, DIFFUSE, TEX1), stride 0x24, type 1 strip / else list (RVA 0x16FDB0..0x16FE35).
Per part (record 0xC0 bytes): name key RVA 0x39810; file-scope search RVA 0x3AD40 (first kind-0x20
via RVA 0x72430, then node chain comparing 16 bytes at node +0x31, i.e. IMG payload +1), then
RVA 0x3ACD0 (RVA 0x3AC80 full 16-byte match on the global texture list, then bytes 8..15). Part
flags dword after the name (RVA 0x1701D4..0x170216): low 28 bits vertex count, bit 31 → record
+0x20, bit 29 → record +0x24.

Draw RVA 0x1844C0 (Direct3D 8 device calls through the device vtable): LIGHTING 1 at RVA 0x184556;
environment lights RVA 0x188B70 (two) or RVA 0x18B9A0 (one), ambient RVA 0x18B9E0, fog
RVA 0x18BA20, the same four builders the zone pass RVA 0x17B510 (called from RVA 0x178CCE, also
LIGHTING 1 at RVA 0x17B818) uses; SetMaterial at RVA 0x184737; stage 0 COLOROP MODULATE2X,
ALPHAOP MODULATE4X, args TEXTURE / CURRENT (RVA 0x18476C..0x1847F6); ALPHABLENDENABLE 0 at
RVA 0x1848D0, then per part = bit 31 (RVA 0x184B48), SRC/DEST blend 5/6; alpha flag →
ALPHATESTENABLE 1, ALPHAFUNC 5, ALPHAREF 0x60 (RVA 0x1848FA..0x18492A; the zone pass sets the same
pair at RVA 0x17B7BF..0x17B7DF); CULLMODE 3 from the zeroed record word, per part NONE when bit 29,
else 2/3 by renderer +0x390FC (RVA 0x184AB9..0x184AF9). Light `l_01`: RVA 0x1844DE tests the Mog
House manager global .data 0x6631D4 (written at RVA 0x94322), looks the name up in the named-light
table .data 0x632040 (256 × 0x70, RVA 0x178760), SetLight wrapper RVA 0xBF30 and LightEnable on
light index renderer +0x394B0, disabled again at RVA 0x184C49.

Entity path (A2, 264..299): RVA 0x1CD618..0x1CD6AE: look of 9 words [n, 0 × 8] plus a zero
model-kind word and flag byte, descriptor built by RVA 0xABC20(empty name .data 0x3D3C84, look),
created by RVA 0xAB2F0 → RVA 0xAB2A0 (0xCBC-byte character-model object, ctor RVA 0xAB360, vtable
.rdata 0x32ECB0, entity kind byte +0xEE = 2) at (0, 0, 0); appearance RVA 0xAB9A0 writes the look
words to entity +0xFC.. and the model-kind byte to +0xEF; entity flag +0x12C |= 0x400. Set-position
RVA 0x1CDD90 and set-rotation RVA 0x1CDF50 dispatch on the object's handles (+0x5A mannequin, +0x54
static model, +0x56 this entity) to RVA 0xAB830 / 0xAB880 (character) or RVA 0xC3FF0 / 0xC4050
(static model). Compare the
s2c 0x00E handler RVA 0x9B550 (registered with id 0x0E at RVA 0x9443C): it switches on packet byte
0x30 & 7 (jump table .text 0x9CE88); case 0 (standard) at RVA 0x9C99C sets entity +0xEE = 2, +0xEF
= 0 (3 only for model ids 531..552) and +0xFC = packet word 0x32.

Placement: set-position RVA 0x1CDD90 (origins .data 0x37D7B4 via RVA 0x1CCE50; constants .rdata
0x329A30 = 0.6, 0x337E28 = 0.1, 0x32B15C = 0.3); set-rotation RVA 0x1CDF50 (0x32AB98 = π/2, Euler
(0, r·π/2, 0)); bounds .data 0x37D7D4 via RVA 0x1CCE60, clamp RVA 0x1CCEC0. Wall: position tables
via RVA 0x1CCE90 (.data 0x37D9C8 1F, 0x37DA18 1F when manager +0x26 == 0x121, 0x37DA68 2F),
rotation tables via RVA 0x1CCE80 (.data 0x37D960 / 0x37D968), neighbour tables via RVA 0x1CCE70
(.data 0x37D7DC / 0x37D7F0); my compact 2F rule set was checked against all 92 neighbour entries
(0 mismatches). Manager built at RVA 0x9431D with (MyroomMapNumber, floor) → +0x26/+0x28.

Plant: RVA 0x1CDA60 (extdata +0x10 Step, +0x14 Kind), table read RVA 0x1D5EC0, 999 check, plant
attached at object +0x64, y − 0.3.

Visibility: predicate RVA 0x1D5FE0 = game state +0x40DB1 bit 0x10 clear; bit set at RVA 0x942A6
from RVA 0xF9650(floor, MyroomMapNumber) (floor != 0 or 0x121..0x123) when zone flag 0x100 (set by
the s2c 0x00A handler RVA 0xFACF0 for LoginState 1/4/5, jump table RVA 0xFB640). Hide RVA 0x859D0
called from ctor RVA 0x1CD78C and update RVA 0x1CE836; mode 1 body skipped at RVA 0x1D3530;
per-frame skip RVA 0x1D4D9D. NPC rule: item table .data 0x3383D8 (286, 287, 426), model word .data
0x338458 = 0x737, RVA 0x1D4BA0 / 0x1D4BE0 / 0x1D4C80. Scan RVA 0x1D2B10 (existing-object branch
RVA 0x1D2BC7, look compare RVA 0x1CDD60), runs from RVA 0x1D4B40.

Editor: manager tick RVA 0x1D4B79..0x1D4B90; browse/camera RVA 0x1D4D90 (mode-change block
RVA 0x1D4DA3, focal table .data 0x37EB94, player point table .data 0x37EBA8, camera setter RVA
0x152B0 → camera +0x2F8, FOV formula RVA 0x15490 with k 0x329D40 = 192, default 350 at RVA
0x153D3; directions RVA 0x1D4F55..0x1D4FC3, Enter RVA 0x1D5023 (actions 0x8F/0x8B) → RVA
0x1D5EF0, Esc RVA 0x1D5063 (action 0x0F) → RVA 0x1D3380(0, −1, 1), view cycle RVA 0x1D50AA
(action 0xA7) → RVA 0x17A590 (entries with id 999)). Navigation RVA 0x1D5710 (0x32A378 = 0.1).
Camera RVA 0x1D4080 (limits .data 0x37EBC8 via RVA 0x1D6030; 0.7 at 0x32D698, 4.0 at 0x32A3A0,
±5.2 at 0x3384BC/0x3384C0; distance RVA 0x1ECB0). Cursor sprite RVA 0x1D3F60 / 0x1D3FC0 (sprite
group RVA 0x1C8C30(6)), drawn from RVA 0x1D5680. Pulse RVA 0x1CE9E0 / 0x1CE900, called from
per-object update RVA 0x1CE7A0. Action query RVA 0x123A70(ctx, action, mode, −1): mask table .data
0x36CF60 vs current context .data 0x36E3A0; default keys .data 0x36D6B0 (3 u16 per action, low
byte DIK code, high byte modifier class: 0x80 no Ctrl/Alt, 0x40 not typing, 0x10 Ctrl/Alt chord +
flag .data 0x36CF5C); key state RVA 0x123380 (mode 0 = bit 0 press+repeat, mode 1 = bit 1 press,
set by RVA 0x5CF7..0x5D7A). Actions: 0x31/0x32/0x33/0x34 Up/Down/Right/Left, 0x8D Num−, 0x8E none,
0x8F Enter, 0x90 Esc, 0x0F Esc, 0xA7 Num5/V. The arrow-axis float getters RVA 0x1223D0..0x122430
(dispatcher .data 0x36D0D8, read by RVA 0x123970) are not used for furniture movement.

Moving: input RVA 0x1CE360 (confirm 0x8F → validity RVA 0x1D5DC0 → RVA 0x1CF0A0, rebuild mode 5
RVA 0x1CCF50 sets dirty +0x2C; cancel 0x90 → mode 6 or +0x2E; directions via remap RVA 0x1CD060
(config 0xC1 via RVA 0x193850, camera eye.z +0x4C vs focus.z +0x58, remap .rdata 0x337E04 =
2,3,0,1) → RVA 0x1CD0A0 → step RVA 0x1CD150 (children RVA 0x1CD290, level loop RVA 0x1CD200);
rotation RVA 0x1CE512..0x1CE689, child transform RVA 0x1CD2D0). Start move RVA 0x1D37E0 (tint
0x80606060 via RVA 0x1D39A0, state 2 via RVA 0x1CE1E0 unregisters via RVA 0x1D5EA0). New piece
RVA 0x1D39E0 → ctor uninstalled branch RVA 0x1CD7AF. Sounds RVA 0x1D56A0 (names 8058/8059/8060,
kind 0x3D, file at manager +8); menu sounds RVA 0x37B30 / 0x37B90 / 0x37BF0 / 0x37C50 (Sep chunks
0001..0004). Overlay RVA 0x1CF110 → cells RVA 0x1CF350 / 0x1CF410 (offsets 0x32A85C = 0.04,
0x32D42C = 0.06, inset 0.05), wall RVA 0x1CF4D0 / 0x1CF510 (corner table .data 0x37E248), colour
bytes swapped into the vertex dword at RVA 0x177BB1..0x177BE0.

Grid: object at manager +0x14; cells 20 × 26 × 8 bytes (slot, stacked slot, container, stacked
container, flags 1 floor-free / 2 top-free / 0x80 blocked, u16 height) init RVA 0x1CECF0 (1F block
at +0x4E0), wall slots 92 × 5 at +0x1040; occupy RVA 0x1CED80, release RVA 0x1CEEF0, test RVA
0x1CEFD0, link rebuild RVA 0x1CEC50, surface height RVA 0x1CEC20.

Windows: rmlo1 handler vtable .rdata 0x33A6D0 (open RVA 0x1F44F0, close RVA 0x1F4530, input RVA
0x1F4750), rmlo2 .rdata 0x338290 (open RVA 0x1F4540, input RVA 0x1F47F0, table RVA 0x1F48C4),
rmlo4 .rdata 0x338248 (open RVA 0x1F45D0, input RVA 0x1F48E0, table RVA 0x1F49E0); registry
entries rmlo1/2/4 flags 0x00021000. Item state setter RVA 0x118B60 takes (state, id). Storage
tests: list count byte of the container-2 list object, size RVA 0xEB960(2), piece storage sum RVA
0x1CE130 (item info +0x2A). Openers RVA 0x1D5EF0, 0x1D3684..0x1D36B2. Put away RVA 0x1D3C40,
all RVA 0x1D3CC0, flag RVA 0x1CE0F0, contents test RVA 0x1CE170; dialogs RVA 0x210E30 / 0x210F10
with GetText RVA 0x152EF0(12, 0x35) and (14, 0x267). Shelf picker handler RVA 0x1D6C90 / 0x1D6D10
/ 0x1D6CD0; callbacks RVA 0x1F4A00 / 0x1F4A50 / 0x1F4AA0; list mode vtable .rdata 0x33A668
(row colour RVA 0x1F4270, select RVA 0x1F42E0, counts RVA 0x1F41A0, limits via RVA 0xA1860(0x62 /
0x63)); RVA 0xA1860 prints through message table 6 = language slot 0x68 (EN file 7031). Mode
switch RVA 0x1D3380 (help/title RVA 0x1D361E..0x1D364D on `titlewin`/`helpwind`). `/layout` RVA
0x7E630 (message 0xBC RVA 0x7E677, message 5 RVA 0x7E729, buzzer RVA 0x7E754, message 6 RVA
0x7E76F, sysmes 0x36 RVA 0x7E77D). Encoders RVA 0xF0CC0 / 0xF0D40 / 0xF0C80, flushed at RVA
0x1CE7A9..0x1CE801 per object from RVA 0x1D5140 (containers 1 then 9, RVA 0x1D5120); destroy and
reselect RVA 0x1D517F..0x1D52A7 (nearest RVA 0x1D5CD0).

List rows: the item window skips a row whose mode colour is 0 (RVA 0x1F0F3B / 0x1F15DB). Shelf
picker open RVA 0x1D6CD0 greys item 2 from RVA 0xF97A0 (game state +0x3C43A bit 0, written by RVA
0x9D096 in the s2c 0x067/0x068 handler RVA 0x9CEE0; the same handler writes the message-188 value
+0x40C3C via RVA 0xF9620). Reselect after removal: position taken at RVA 0x1D51DF (selected piece),
rule `new = (sel != nearest) ? nearest : 0` at RVA 0x1D5292. Mog Garden skip of the scan: RVA
0x1D4B35 (ZoneNo 0x118 test RVA 0x1D6000).

Own scratch scripts and dumps live under the session scratchpad `reader_kit/mogarr/`; no source
edits, commits or runs.
