# Walker: the per-frame player update, its speed law and gait scale, 2026-10-25

How a retail client frame turns movement intent into an actual position change and a gait. Read from
the installed client's whole `FFXiMain.dll` recompiled to C (KNOWN_CLIENTS row `retail-2026-10`, DLL
build 2026-10-05). No live client session was run; nothing here came from the screen.

## Frame order and gate

One unit does the whole player step, driven by the scene drivers. It starts with a gate: if the UI is
in a state that drops control, or there is no linked object yet, or the actor reports it cannot move,
the walker zeroes its two internal fields (auto-run latch and gait scale) and returns without moving.
It then reads the two movement axes through the action dispatcher, flips the sign of lateral so that
positive means right, publishes its per-frame intent to the UI singleton, and picks up heading from a
single actor field that five separate sites store to (so turning is not exclusive to walking).

## Speed law

The rate is the speed byte the server sends in each entity update: LSB writes one byte for the travel
rate and a neighbouring byte for the animation rate into the moving-object header of every carrier
(vendor/server/src/map/packets/entity_update.cpp CEntityUpdatePacket::updateWith, and the same pair in
vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_POS_HEAD Speed / SpeedBase). The client copies that
field into its actor record and spends it as world units per second; it does not re-derive it from a stat.
Kuluu decodes the same pair on the receive side and turns it into travel speed directly. The law:

1. Ask the mount/run predicate pair. When it holds, take the server-fed rate and **double it**; otherwise
   spend the candidate rate already chosen for this frame.
2. Clamp the result to **30 world units per second** - a client-side ceiling applied after doubling.
3. Divide by 60 - one tick per render frame at 60 Hz, so a step is "speed over sixty", with no
   delta-time term anywhere in this arithmetic.

One caution belongs with that law: the value doubled when running and the candidate used otherwise are
both *inputs* to this unit; neither is computed here, so the wire-speed semantics live in the server's
update packet fields (vendor/server/src/map/packets/entity_update.cpp Speed and SpeedBase, written by
CEntityUpdatePacket::updateWith) and their client-side POS-head decode (ffxi-proto/src/decode/entity.rs
PosHead speed), not in this unit.
What is now settled (it was left open when this record first landed): the gait scale of the section
"Gait rate selection" is applied to the two axis values *in place*, inside this unit, before the heading
blend reads them. Both scaling call sites lie on every path from the walker's entry to its displacement
blend, and no raw re-write of either axis happens between the last scaling call and the blend; when a
certain per-actor mode byte is set, one scaled axis is moved into the blend's second operand slot and the
first zeroed instead. The blend therefore integrates gait-scaled values: an intent with mid-range length
crosses ground at two thirds of what a full press crosses in the same tick.

That per-tick distance is then spent as follows: forward and lateral components are blended against
heading using sine/cosine of the heading (and of heading minus a half turn for the backward
component), each scaled by the step. Two objects receive the result through interface accessors -
the actor's own movement/ray state block and the scene entry's companion block - so both the actor
and the scene see one consistent displacement per frame.

## Step shortening by self-probe

Before committing, the walker asks itself for room: a query dispatched to its own collision handler
fills a result buffer, and the walk margin is **0.8 times the length of the first and third vector
slots** of that buffer (square root of the sum of their squares, scaled by 0.8). The intended move is
compared against this margin, and a step longer than what the probe allows is shortened or clipped.
The comparison appears at three points in the unit: before moving, after integrating, and again on the
steer tail.

## Gait rate selection

A helper turns intent magnitude into both a playback scale and - per the paragraph above, settled on this
turn - the ground-motion factor. It normalizes the two axis values (below about one millionth of unit
length it treats the pair as zero), multiplies each by 1/length in place, then re-scales them in place by
one of **1.0**, **two thirds**, or **0** from the pre-normalization length against cutoffs: full above
about 0.9, two thirds down to a dead zone at about 1/20 of unit length, zero below it. A per-actor
gait-override byte can raise a scale of 0 back to two thirds (that is how a deliberate walk with no axis
input still moves and animates). The selected scale is also stored into an actor field read by the motion
layer; which clip plays lives in that model motion layer, not here.

## Heading and wrap constants

Heading lives as one float per actor; five stores keep it current (turn, steer-into-motion, target
tracking, and two correction paths). The unit carries its own two-pi, pi, half-pi and degree-to-radian
constants for wrapping and conversions, so heading arithmetic never leaves the walker unnormalized.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| per-tick step is speed divided by 60, with no delta-time term | kuluu/src/view_native/input.rs RETAIL_MOVE_TICKS_PER_SEC | matches |
| one branch of the rate selection doubles the server-fed rate field before the clamp, keyed on the mount predicate | kuluu-snapshot/src/lib.rs MOUNTED_SPEED_MULTIPLIER | matches: 2.0 applied to the decoded packet speed while mounting, then clamped to the same ceiling; both engines feed this doubling from the wire speed byte and nowhere else (this record) |
| the rate is clamped to 30 world units per second after doubling, before tick division | kuluu-snapshot/src/lib.rs MAX_MOVE_SPEED_YPS | matches: same ceiling value (30.0 yalm/s); kuluu applies it when decoding the packet speed rather than after a client-side doubling |
| mounting doubles the movement rate | kuluu-snapshot/src/lib.rs MOUNTED_SPEED_MULTIPLIER | matches: 2.0, keyed on mount state where retail keys its doubling on the run predicate |
| the gait scale (one, two thirds or zero) multiplies the axis pair in place before the displacement blend reads it, so a mid-range intent crosses ground at two thirds of full-press rate | kuluu-snapshot/src/lib.rs WALK_SPEED_SCALE / WALK_SPEED_DIVISOR = 1.5 applied by input.rs move_step_speed_yps; pinned by walk_lock_forward_and_locked_backpedal_share_one_walk_speed | matches |
| control gate zeroes both walker fields and returns without moving | kuluu/src/view_native/input.rs reset_local_movement | matches |
| lateral axis sign is flipped by the walker so positive means right | kuluu/src/view_native/input.rs resolve_move_inputs | matches |
| displacement blends sine/cosine of heading for forward and backward components | kuluu/src/view_native/walker/step.rs heading_blend, fed by kuluu/src/view_native/input.rs dispatch_movement_system from consts::scaled_move_axes axes; pinned by heading_blend_places_each_axis_on_its_own_heading_term | matches (closed 2026-10-25): the travel displacements are now built per component against the aim and its right vector, the backward term read along the reversed aim; kuluu keeps facing as the wire's 256-step byte so sine/cosine of heading enter through that byte's unit vectors. Scope: the locked-on side/backward absolute step lengths (input.rs LOCKED_SIDE_STEP_DIVISOR family, sourced from research/XIClient ControllableActor::ChangeVectorLengthByDirection and BaseActor::GetWalkSpeed) still pre-scale the rate candidates upstream - this record states its rate candidates are computed upstream of the blend unit, so they were kept rather than dropped |
| the intended step is shortened by a self-probe margin of 0.8 times probe distance | kuluu/src/view_native/walker/step.rs ResultVector::margin (consts PROBE_MARGIN_SCALE), measured each tick by `probe_answer` against the authored wall class and applied to the intended step inside `step`, ahead of sweep's wall law; pinned by margin_uses_the_recorded_scale_and_skips_vertical | matches (rescored W3 2026-10-25 after the column walker was removed): a pose whose body query sits inside geometry has its intended step shortened by scale times the measured pair before commit. Kuluu's measurement for the pair is its own body intrusion against authored walls; blocking and sliding stay owned by sweep::sweep, so in normally-swept ground the margin reads zero |
| gait scale is one of 1.0, two thirds or 0 from magnitude cutoffs (full above 0.9, dead zone under about 1/20) | kuluu/src/view_native/walker/consts.rs gait_scale and scaled_move_axes, fed by kuluu/src/view_native/input.rs pose_intent and the travel sites; pinned by gait_scale_reads_its_three_bands_at_the_boundaries and scaled_move_axes_normalizes_before_rescaling | matches (closed 2026-10-25): the axis pair is normalized against its own length and rescaled in place before anything reads it - pose_intent reports the magnitudes to animation, and travel crosses at the scale, not by key state |
| a per-actor gait override raises a scale no larger than one third up to two thirds | kuluu/src/view_native/walker/consts.rs gait_scale (gait_override parameter, raise branch tested) | differs: the law is implemented and pinned (gait_override_raises_only_a_sub_third_scale_to_walk), but every caller passes it false - kuluu has no state established as retail's per-actor override channel, so the raise never fires in play |
| heading kept as one normalized float per actor with wrap constants local to the walker | kuluu/src/view_native/input.rs advance_heading_turn | matches |

## Provenance

Method: generated C recompile of `FFXiMain.dll`, build 2026-10-05, image base `0x10000000`; looked up
per [vendor/rea/INSTALL.md](../../../../vendor/rea/INSTALL.md) as `f_<VA>` with grep over
`research/xi-on-anything/generated/all`, plus `tools/newbuild.py dis --label 2026-10-05` where a
listing needed confirmation. Full working notes: `research/xi-on-anything/movement-map.md` sections 1,
2 and 3b (untracked). Findings carry provenance "generated C, build 2026-10-05".

Walker unit and drivers:

- Unit `f_100a6240`, covering `0x100a6240..0x100a779f`; caller `f_100a5ec0` at site `0x100a5edf`,
  which comes from `f_100a57f0` and scene drivers `f_100c63d0`, `f_100ac4e0`, `f_100c47f0`. Camera
  update runs from inside the walker at site `0x100a66d3`.
- Early gate sites `0x100a6570..0x100a65c8`: reads f32 triple `[actor+0xE4]/[0xE8]/[0xEC]` and dword
  `[actor+0xF0]`; requires byte `[[0x104568fc]+8] != 0`, `[actor+0x70] != 0` and NOT
  `f_100964f0(actor)`; zeroes `[actor+0x598]` and `[actor+0x594]`.
- Axis gate: predicate `f_10158aa0([[0x1057876c]])` plus byte `[[0x1057876c]+0x22]`; action 4 read at
  site `0x100a6603`, action 5 at `0x100a6610` with a sign flip; steer-tail second axis read `0x100a70f0`.
- UI feedback writes `[ui+0xa8]` and `[ui+0xac]` at sites `0x100a6623`, `0x100a6629`.

Speed law and integration:

- Speed block sites `0x100a64d2..0x100a650c`: run predicate `call f_10084390` then
  `call f_10084370`; base speed through accessor `[vtable+0x1a0]` (`f_10082070`, returns
  `[obj+0x98]` as float, on handle vtable `0x1032e890` and its six sibling interface vtables);
  doubling is a self-add; clamp constant f32 `[0x10329fd8]` = 30.0; tick divisor f32 `[0x1032b160]` =
  `0.0166667`; per-tick result at `[esp+0x28]`.
- Displacement blend sites `0x100a753c..`, with heading from the stack; move-block writes through
  `f_10027120` at site `0x100a75bb`; blocks resolved via singleton getter `f_10081550` (singleton
  dword `0x10487f74`) and interface accessors `[obj+0x1bc]` (`f_100a4740`, returns `&obj + 0x5FC`).
- Self-probe wrapper `f_100aaef0`: allocates a 0x2c-byte result buffer, dispatches interface accessor
  `[vtable+0x1c8]` whose handler is `f_100d4800`, then scales by f32 `[0x10329a34]` = 0.8 over the
  square root of buffer slots at `[buf+0]` and `[buf+8]`. Call sites in the walker: `0x100a62bc`,
  `0x100a75f2`, `0x100a7602`; compare against intent via a subtract-and-compare at `0x100a75f7`.

Gait scale:

- Helper `f_100a78d0`, called twice from the walker at sites `0x100a668a` and `0x100a7174`. Both push
  three frame-slot addresses in order (&frame+0x50, &frame+0x14, &frame+0x10); the prologue loads
  `[esp+0xc]` into esi (= &frame+0x10) and `[esp+0x14]` into edi (= &frame+0x14), and only those two are
  ever written (`ret 0xc`). The first-pass normalization: L = fsqrt of the squares, compared against f32
  `[0x1032a42c]` = 9.9999997e-07; at or below it the pair is multiplied by f32 `[0x103295d8]` = 0.0, above
  it by `fld [0x1032961c] (=1.0); fdiv st(1)` = 1/L. Scale selection compares L against f32 `[0x1032c9a4]`
  = 0.9 (via the C3|C0 idiom `and eax,0x4100`), then against f32 `[0x1032a3e0]` = 0.05; constants written
  are `0x3f800000`, `0x3eaaaaab`, `0`. Bump: accessor `[vtable+0x338]` (`f_100a4690`, byte flag at
  `[obj+0xFA]`) true and scale <= f32 `[0x1032a84c]` = one third raises the scale to `0x3eaaaaab`. The
  chosen scale is multiplied back into both slots in place at sites `0x100a797f..0x100a798d`, and its raw
  bits are stored to `[actor+0x594]`.
- Blend operand identity (settled this pass): the displacement blend reads frame slot +0x14 at sites
  `0x100a7548`/`0x100a7568` and slot +0x50 at `0x100a7554`/`0x100a7574`, then multiplies the sums by the
  per-tick step in `[esp+0x28]`. Slot +0x50 has exactly two writers: zero-init at `0x100a65f1`, and the
  slot move at `0x100a729d..0x100a72ad` (`eax=[frame+0x10]; [frame+0x10]=0; [frame+0x50]=eax`) taken when
  accessor `[vtable+0x340]` returns true. Raw axis writers after call A: action-4 getter store to +0x14 at
  `0x100a7118` and constant `0x3f800000` at `0x100a68fb`; both precede call B, which re-scales.
- CFG audit over the walker unit (jump-edge scan): the only edge from between the two call sites to past
  call B is `0x100a7185 -> 0x100a728f`, which itself executes after B; both branches around B (`jnp
  0x100a7163` and fallthrough via the helper pair at `0x100a715c`) land on B. Hence every path from entry
  to blend passes through a scaling call after any raw axis write.

Heading:

- Stores `fstp dword ptr [esi+0xE8]` at sites `0x100a636b`, `0x100a6457`, `0x100a6988`, `0x100a705b`,
  `0x100a709d`. Wrap and conversion constants: two-pi `[0x10329d2c]`, pi `[0x10329d30]`, half-pi
  `[0x1032d430]`, degree-to-radian `[0x1032a9f4]`.

Actor fields written or read: base speed f32 `[actor+0x98]`; heading f32 `[actor+0xE8]` (with the
neighbouring triple at `+0xE4/0xE8/0xEC`) and dword `[+0xF0]` as gate inputs; byte flags `[+0xF9]`
free-run, `[+0xFA]` gait override, `[+0xFB]` parallel move (interface accessors `[vtable+0x330]`,
`+0x338`, `+0x340`).

Could not resolve statically: the interface vtable of the concrete scene-entry instance behind
`[[0x104568fc]+0x50]` is chosen at runtime during init (`f_10001240`, `f_100013c0`, `f_10001520`) -
immaterial because all seven interface classes in the image share these handlers; and inside the steer
tail, the exact order of auto-run cancel branches beyond the free-run/parallel flags and backward
deflection.
