# Movement input: the action table, its pump and the auto-run latch, 2026-10-25

How the retail client turns devices into movement intent. Established by reading the installed
client's whole `FFXiMain.dll` recompiled to C (KNOWN_CLIENTS row `retail-2026-10`, DLL build
2026-10-05). No live client session was run; nothing here came from the screen.

## The pump

Exactly one pump runs per frame, and it is the first thing input handling does: it opens by latching
the frame clock, then pulls device state. It has a single call site, so there is no second path and
no way for a consumer to read intent without the pump having run that frame.

Per frame it acquires each runtime-opened device and takes a 256-byte keyboard image plus two
capability blocks of twenty entries each. When a device reports it was lost, is unconfigured or is
otherwise unavailable, the pump zeroes every button bit-field and re-centers all analog axes to their
neutral value - so an unplugged pad stops driving the character dead rather than latching the last
direction it saw.

The pump also manufactures digital buttons from analog movement: when a stick's axis leaves a dead
band of plus or minus 63 counts, it ORs one bit of a pair into each of the pressed fields (one pair
per axis family). Those synthesized bits are what make "tap the stick" behave like a key press.

## The device state record

Consumers never see raw devices; they read named fields the pump maintains:

| field | width | semantics |
| --- | --- | --- |
| held pair | two 32-bit masks | still-down set, tested by the analog-button actions |
| edge-latched set | 32-bit mask | pressed-and-unconsumed; a test re-stores the masked value and clears a companion field at the same time, so a press is handed out once |
| pressed-this-frame pair | two 32-bit masks | went down during this frame's read |
| analog axes | four bytes | neutral at 128, scaled by one over 128 into a signed unit range |

## Action dispatch: numbers in, signed unit float out

Consumers ask for an **action number** and get back `-1.0`, `0.0` or `+1.0`; the analog axes return
the continuous normalized value instead. The table has **54 numbered entries**, one hole inside that
range, and entries past it are absent (not zero-filled). Each entry is a pair: an analog getter plus
a digital fallback used when the device cannot deliver an axis.

Two gates sit in front of every lookup:

- A per-action suppression mask. A set bit removes that action globally - this is how key rebinding
  and disabled contexts mute input without touching consumers.
- A player-control gate. When it is clear, only the two **movement axes** (forward/back and lateral)
  are forced to neutral; everything else still reads normally. That is why open menus, dialogs and
  cinematic states stop the character without needing per-window filters: the axis read itself goes
  dead.

Action groups, by what each tests rather than by bit values:

| actions | getter kind | tested state |
| --- | --- | --- |
| 0 to 3 | four related families | one designated bit of the pressed-this-frame fields |
| **4** (forward/back), **5** (lateral) | analog axis, digital fallback | two of the four analog bytes; the walker reads these two and flips the sign of lateral itself |
| **6**, **7** | analog axis, digital fallback | the other two analog bytes - consumed by the camera unit as orbit horizontal and vertical |
| 8 and 10 / 9 and 11 | paired bit test | one mask pair selects plus/minus, so each pair is a two-way switch (plus one or minus one) |
| 13 to 52 with gaps | single-bit key tests | one designated bit of the held fields or of the edge-latched fields; several numbers deliberately alias the same test (for example four separate numbers share one bit, and tail rows re-bind the movement and camera getters onto extra numbers) |

The digital fallbacks for the four axes gate on interface mode, a busy state and a menu-open
predicate before falling through into the button pumps, and they can push virtual-stick buttons from
a software stick instead of the physical one.

## The auto-run latch lives inside the walker

Nothing outside the movement code can turn auto-run on or off: across the whole DLL, the latch field
and its companion gait-scale field are written **only** by the walker unit and its two helpers
(gait selection and target tracking). External code has a read accessor and no setter. Auto-run is
therefore intent-driven - once forward intent latches it survives steering, and free-run / parallel
flags plus backward-deflection branches cancel it. When the movement gate closes, the same gate that
guards the walker zeroes both fields, which is what "auto-run stopped by a dialog" actually is.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| one pump per frame, latching the frame clock first, with no second path to intent | kuluu/src/view_native/input.rs handle_input_system | matches |
| consumers read numbered actions returning minus/zero/plus one, axes continuous | kuluu-render/src/keybinds/mod.rs Action | differs: kuluu keys are named booleans per consumer, there is no shared numeric action table |
| device loss or unavailable state zeroes the button fields and re-centers every axis | kuluu-render/src/keybinds/mod.rs Bindings | missing: winit delivers discrete key events, so there is no stale-held-device path to clear |
| a per-action suppression mask mutes an action globally | kuluu-render/src/keybinds/mod.rs ActionMask with Bindings::mute_action, unmute_action and action_muted, consulted inside every dispatcher read path (pressed, just_pressed, matches_logical, matches_keycode); pinned by a_muted_action_is_dead_for_every_consumer and every_action_has_its_own_mask_bit | matches (closed 2026-10-25): one bit per action in front of every lookup, so a muted action reads dead for all consumers whatever it is bound to; the mask width is guarded against the action set growing past one word |
| movement axes go neutral when the local player is not under control, without per-window filters | kuluu/src/view_native/input.rs mode_cancels_autorun | matches: input mode gates both axis resolution and auto-run centrally |
| a press is handed out once (edge-latched set consumed by mask rewrite) | kuluu-render/src/keybinds/mod.rs Bindings::just_pressed | matches |
| the auto-run latch is written only by movement code, never by an external setter | kuluu/src/view_native/input.rs AutoRun::toggle_phantom_forward, AutoRun::cancel_phantom_forward | matches: the latch has no public setter; every change is one of these named transitions and both live in the movement module (this record) |
| analog stick deflection beyond the dead band (plus or minus 63 counts of an axis neutralled at 128) synthesizes a digital press, one per axis and sign, for tap-style handling | kuluu/src/view_native/gamepad_input.rs stick_press_edges | matches: rising edge across that band in the pad pump, published as PadStickIntent::presses; the axes themselves stay analog through pick_mag (this record) |

## Provenance

Method: generated C recompile of `FFXiMain.dll`, build 2026-10-05, image base `0x10000000`; looked
up per [vendor/rea/INSTALL.md](../../../../vendor/rea/INSTALL.md) as `f_<VA>` with grep over
`research/xi-on-anything/generated/all`, plus `tools/newbuild.py dis --label 2026-10-05` where a
listing needed confirmation. Full working notes: `research/xi-on-anything/movement-map.md` sections 4
(untracked). Findings carry provenance "generated C, build 2026-10-05".

Pump and device fields:

- Pump `f_10005a40`; sole caller inside `f_10011a00`; frame clock call at the head of the pump.
  Device object global `dword 0x104e1d44`, input/UI singleton `dword 0x104e1d4c` (mode byte at `+0x4d`;
  walker writes `[ui+0xa8]` and `[ui+0xac]`).
- Device acquisition through COM vtables of runtime-created DirectInput objects: `[dev+0x24]`
  GetDeviceState with a `0x100`-byte buffer at `[dev+0x98]`; `[dev+0x28]` variant query, count `0x14`,
  into blocks `[dev+0x79c]` / header `[dev+0x8dc]`. Error codes `0x800700aa`, `0x8007000c`,
  `0x8007001e`; zero-and-recenter sites `0x10006055..0x100060e4`.
- Axis composition (byte, `+0x80` centering) at `0x100067f1`, `0x10006803`, `0x10006815`, `0x10006828`;
  second path `0x10006..0x100071e0` (`0x100071a9..`); re-center sites `0x100060be..0x100060e4`.
- Synthesized button bits OR'd at `0x10006836..0x100068d9`: bits `0x10`/`0x20` for one axis pair,
  bit `0x80` for another, thresholds +-`0x3f`, destinations `[dev+0x228]`, `[dev+0x238]`, `[dev+0x220]`.
- Fields: held pair `[dev+0x220]/[dev+0x224]`; edge latch `[dev+0x228]` with companion clear at
  `[dev+0x22c]`; alternate held pair `[dev+0x230]/[dev+0x234]`; pressed-this-frame `[dev+0x238]`
  (companion `[dev+0x23c]`); analog bytes `[dev+0x250..0x253]`, scale dword `f32 [0x1032a778]` =
  `0.0078125`.

Dispatch and table:

- Getter dispatcher `f_10123970`: suppression mask `[0x1036e3a0] & (1 << bit)`; player gate byte
  `[[0x104dfd98]+0x4194]` zeroes actions 4 and 5 when clear (`cexi_player` in the build meta);
  getter table `[eax*8 + 0x1036d0d8]` with dword 0 = analog getter and dword 1 (`+4`) = digital
  fallback; static length read from the unpacked image - null from index 54, hole at index 12.
- Unit return constants: `+1.0 [0x1032961c]`, `-1.0 [0x1032a3f0]`, `0.0 [0x103295d8]`.
- Axis getters `f_10120c10` (axis byte at `[dev+0x252]`), `f_10120c40` (`[dev+0x253]`), `f_10120c70`
  (`[dev+0x250]`), `f_10120ca0` (`[dev+0x251]`); their fallbacks `f_10122e20` (pushes 1 into
  `f_10122e30`), `f_10123030` (pushes 2), `f_101232e0`, `f_10123450`.
- Paired bit testers `f_10120cd0` (bits `0x40` / `0x80`) and `f_10120d20` (bits `0x10` / `0x20`) over
  `[dev+0x220]/[dev+0x224]`. Key-test wrappers by bit: bit `0x1` `f_10122250`; `0x8` `f_10120f30`;
  `0x4` `f_10120d70`; `0x100` `f_101210f0`; `0x8000` `f_10121ca0`; `0x2` `f_10121000`; `0x1000`
  `f_10121420`; `0x200` `f_10121200`; `0x2000` `f_10121530`; `0x10000` `f_10121860`; `0x400`
  `f_10121310`; `0x4000` `f_10121640`. Entry shims: actions 13/14 -> `f_10122250`; 15 -> `f_10122270`;
  16 -> `f_101222f0`; 17/34 -> `f_10122290`; 18/21/28/31 -> `f_10122330`; 19/20 -> `f_10122310`;
  22/33 -> `f_10122350`; 23/32 -> `f_101222d0`; 24/25/37/53 -> `f_10122390`; 26/30/36 ->
  `f_101222b0`; 27/38/47 -> `f_101223b0`; 48 -> `f_10122370`; actions 0..3 -> `f_101223d0`,
  `f_101223f0`, `f_10122410`, `f_10122430` over case-4 families `f_10121db0`, `f_10121f90`,
  `f_10121ea0`, `f_101220a0`. All wrappers share one code shape with device-family jump tables (for
  example `0x10120f0c`, `0x1012172c`) selecting held (`+0x220`/`+0x230`) or pressed-with-rewrite
  (`+0x228`/`+0x238`, clear `+0x22c`).
- Fallback gating: UI mode byte `[[0x104e1d4c]+0x4d]` (branches at values 4 and 5), busy state
  `[[0x1057876c]+0xc4]`, menu predicate `f_101b5c90(this=[0x10662d40])`; tail jumps into button pumps
  `f_101260c0`, `f_101260d0`, `f_10125fe0`, `f_10126190`; virtual-stick ids `0xa0..0xa3` pushed with
  device index `0x3f` into `f_10123a70`.

Auto-run:

- Latch `[actor+0x598]` and gait scale `[actor+0x594]`: written only inside walker `f_100a6240`, gait
  helper `f_100a78d0` and target-track helper `f_100a7b80`; read-only accessor elsewhere is
  `f_1005e5e0`. Gate that zeroes both: sites in the walker's early exit (`0x100a6570..0x100a65c8`).

Could not resolve statically (all are pointers loaded from runtime state, per INSTALL.md step 5):
the DirectInput device COM interfaces acquired inside the pump; and within the walker steer tail,
the individual latch/cancel branch order was not itemized beyond the flags named above.
