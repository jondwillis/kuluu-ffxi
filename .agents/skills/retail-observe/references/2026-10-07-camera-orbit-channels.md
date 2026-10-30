# Manual camera orbit and body-facing reference

Observed 2026-10-07. Scope: clean retail-2026-09, patch 30260904_1. Independent binary-reading observation; no retail session or Kuluu runtime was driven. This establishes separate channels and a bounded branch contract. It does not establish complete normal-default, lock-on, first-person, or spring-setting policies.

## Manual orbit input

The player-following camera reads the horizontal camera action6 and vertical camera action7 separately from the body-facing reference. Action6 may be supplied by the analog device or keyboard callback. Horizontal/vertical reversal configuration applies to these action values before camera motion. Some menu/control conditions suppress reading these actions; the full eligibility mapping is outside this record.

For horizontal camera action h and the client's current frame-tick multiplier t, the direct orbit angle is h * t * 0.027924444526433945. A camera-follow mode flag changes its scale: when that flag is false, multiply by 6/max(distance(eye,focus),0.01); when true, retain the unscaled angle. This distance is the three-dimensional eye-to-focus length, not only horizontal boom length. In the latter mode a positive transition timer suppresses the direct angle. A separate actor lateral/parallel-control flag suppresses this direct orbit block entirely.

The orbit applier defines theta = atan2(focus.z-eye.z, focus.x-eye.x) and horizontal r = sqrt((focus.x-eye.x)^2+(focus.z-eye.z)^2). After adding the angle, it writes eye.x = focus.x-r*cos(theta) and eye.z = focus.z-r*sin(theta). Horizontal radius is preserved, apart from its degenerate-radius handling. For example, with eye=(0,0,6), focus=(0,0,0), and a small positive direct angle, eye.x becomes negative and eye.z remains positive. This specifies the retail Cartesian convention; an implementation using a different yaw convention must translate the sign rather than copy it blindly.

When no body-facing reference is active, the direct horizontal camera action is not negated into a substitute body reference. Positive and negative manual orbit remain their respective direct polar angles through this path. A product boolean named spring cannot by itself justify reversing the manual action channel.

## Body-facing reference channel

A body-facing event that changes the actor heading stores a separate reference equal to that event's heading-change term and marks it active. The observed turning path replaces this reference; it does not accumulate a history of manual camera input. Several steer-zero/no-turn branches call the same setter with inactive/zero. A reset path also clears it. No timer-based lifetime for this reference was found in the setter/consumer: it lasts until another producer replaces or clears it.

The camera first applies the direct manual horizontal orbit. Only later can it apply the body-facing reference. The observed reference consumer requires all of these predicates: a particular mouse/control configuration is active, the actor camera-follow mode flag is true, the body reference is active, and the horizontal camera action value is nonzero. It then uses the negative of the stored body-heading reference. A remaining transition gate can force that angle to zero. The full route that makes action6 nonzero in this configuration, and its relation to movement steering, were not traced; therefore this is a conditional contract, not a general rule that every body turn causes camera spring-back.

The consumer contains a 6/max(three-dimensional eye-focus distance,0.01) scaling branch when the actor mode predicate is false. The same mode predicate was already required true to enter this consumer, and its concrete getter has no side effects. The observed stable actor path therefore skips that distance scaling for the body-reference consumer. The direct manual path does use that scaling when its mode predicate is false. Applying the distance-scaled body-reference expression universally would not follow the stable branch predicates actually observed.

## Priority, applicability and missing laws

Established: direct manual action and body-facing reference are distinct inputs; direct manual orbit precedes the separately gated body-reference correction; no active body reference means no such correction; the facing producer replaces/releases its own reference rather than persisting manual yaw; coordinate convention and three-dimensional scale distance are as specified above.

Material gaps: mapping the camera-follow flag and lateral-control flag to normal/default versus lock-on versus first-person gameplay; relating the observed mouse/control configuration to a user-facing retail camera-spring setting; exact menu/gamepad/mouse arbitration; producer order relative to camera update across an entire frame; reference behavior on zone/session transitions; lock release priority and lifetime; selected-device polarity as perceived through Kuluu mouse input; behavior in other client builds. The branch trace is insufficient to invent these policies or recommend the entire PR985 camera overhaul as merge-ready. Narrow manual-channel preservation can be tested independently, but broader mode or priority integration needs a separate applicable record and rendered runtime evidence.

## Pull-in: a state-block query into the same engine (re-established 2026-10-25)

Camera pull-in away from geometry is **not** a dedicated ray cast. The camera unit keeps its own eye
state block (four floats) beside it, and each frame it:

- reads that state block through the same controllable-object accessor movement uses, taking two points
  or offsets out of the first three float slots plus one more from a second read of the same block;
- shortens one of those values by one world unit;
- copies the camera unit's own placement (a transform helper on an offset sub-object) so the test runs in
  world space;
- assembles a query descriptor on the stack and runs it through the **candidate-instance collision
  engine** described in the movement-time collision record - same list of candidate shapes, same
  per-shape test method;
- shortens the eye position by distance work (multiply plus square root over hit distances) and merges
  results with the element-wise vector helpers.

Three further query invocations per camera frame go straight to the shared query handler with a caller
buffer instead of going through the state block. So pull-in is one more consumer of one collision engine,
reached from a state block rather than from the actor's own pose, and it shares the readiness gate
(registered handle, resolved object, no pending word) that every candidate in that engine needs.

The September addresses for the update unit and its sub-laws were re-verified on the 2026-10-05 build by
behavior at each site (see the provenance addition below): nothing moved.

## Chase-camera leash and spring (established 2026-10-27, generated C pass)

The followed-camera update does not recompute the on-screen rig from the player each frame. The camera keeps a
pair of persisted world triples - the same pair whose three-dimensional separation scales the manual-orbit
angle (eye-to-focus distance) - and each frame the followed actor's pose flows into one of them through two
chained exponentials:

1. **Candidate triple** - a copy of the followed actor's committed position block, taken through the same
   accessor movement uses to commit poses. Before use its vertical component gets one feed float added in
   place: that word comes from this frame's contact-query pass - the same result family the walker consumes
   per the collision record.
2. **Ease stage** - an in-place blend of a persistent ease triple toward the candidate, all three axes,
   with weights one-half and one-half (both immediate floats; the client contains only four such blend sites
   in total).
3. **Spring stage** - a persisted chase triple, loaded from stored state at frame start and written back to
   it at frame end, chases the ease triple by exactly nought-point-two-five of the remaining difference per
   substep (immediate float alpha), iterated **N times where N is an integerized tick-unit framerate
   measure** - substeps run in proportion to elapsed ticks, so wall-clock response is framerate-independent.
   A nonpositive N skips both stages that frame; a mode gate fed by the followed actor's mode value can also
   skip them entirely for certain special modes (the camera then updates exactly).
4. Reset/transition paths write exact values into the stored pair without running the cascade.

Net law: position, aim height and the pull-in anchor all reach the screen through this one lagged response. A
discrete jump of the committed actor pose - stair riser mount, landing settle, server correction - moves the
candidate by the full step but arrives attenuated, an exponential approach with a time constant of roughly two
substeps. This is retail's camera leash and spring; it is why walking up stairs on retail shows no vertical
kick. The cascade covers all three axes; there is no axis split anywhere in this law.

Not established this pass: the writer list for the candidate's vertical feed word (the value is identified at
one site, its producers are not enumerated); which stored triple becomes final eye versus look-at downstream
in the render path; exact seeding of the ease triple at mode entry beyond the reset writes; the tick measure's
units (tick-proportional behavior is what the sites show; whether N counts sixty-Hz or vsync-thirty ticks was
not settled).

## Manual orbit tap dwell (established 2026-10-27, generated C pass)

Alongside the direct angle law above, the followed-camera update carries manual yaw as a pending quantity per frame: after each polar-applier call it adds `horizontal action × frame-tick multiplier × nought-point-one-oh-six-six-six` into an owned pending-angle slot, and while that pending value stands against nought, the update latches the value **ten** into its camera state dword. The slot pair (pending angle and a companion offset beside it) is consumed further down in a difference-and-clamp block against a stored threshold global, and re-clamped once more at a later site. How exactly the applied view angle composes from that pending pair was not resolved this pass.

What grounds this section is the playtest law (user, retail 2026-10-27): rapid alternating keyboard left/right taps - each a fresh press cycle completing inside ten frame ticks - move the followed camera **not at all**. A held press still orbits at the direct angle rate established above; nothing here adds ramping to a sustained hold. Kuluu previously applied every tap instantly (one full angle step per pressed frame), which is the keyboard-channel half of its stair-jackhammer reports.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| direct manual orbit angle is action times frame tick times a fixed rate constant | kuluu-render/src/mouse.rs CAMERA_ORBIT_RATE_RAD_PER_SEC | matches |
| unscaled when the follow-mode flag is set, otherwise scaled by nominal-radius-over-distance | kuluu-render/src/mouse.rs camera_orbit_yaw_rate_rad_per_sec | matches |
| orbit applier preserves horizontal radius and rebuilds eye from polar angle around focus | kuluu/src/view_native/camera_collision.rs rotate_about | matches |
| positive transition timer suppresses the direct manual angle while follow mode is set | kuluu-render/src/mouse.rs manual_orbit_suppressed, applied by orbit_chase_by_axis with mouse_camera_system's remaining CameraTransition seconds; pinned by a_positive_transition_timer_suppresses_the_direct_angle_in_follow_mode | matches (closed 2026-10-25): positive remaining time zeroes the direct horizontal angle while kuluu's follow-mode flag reads set, pitch untouched. Producer scope: this record does not establish what sets retail's timer; kuluu produces it from its one timed following-camera transition (the mode-toggle CameraTransition) and reads zero otherwise |
| pull-in reuses the movement collision engine through a camera-owned state block, shortening one value by one world unit before querying | kuluu/src/view_native/camera_collision.rs resolve_camera | differs: kuluu runs its own camera ray/leash resolution against zone geometry instead of sharing the walker's query |
| eye shortened from hit distances with multiply and square root, merged by element-wise vector add | kuluu/src/view_native/locked_camera.rs short_of_wall | matches: same net law - move the eye to just short of the hit, one world unit of standoff built in (WALL_RAY_RISE) |
| body-facing reference correction is applied only after the direct manual orbit within the frame | producer kuluu/src/view_native/input.rs StanceParams::body_facing (dispatch commits each turn via camera.rs heading_change_term_rad); state and gates kuluu-render/src/camera.rs BodyFacingReference post_orbit_angle; spent by kuluu-render/src/mouse.rs orbit_chase_by_axis after the direct angle, through the same polar applier | matches (closed 2026-10-25): the channel carries one stored heading-change term with its active flag (producer replaces or retires it, no timer), and all four record predicates gate the negative-of-term correction - control-config active, follow-mode set, reference active, manual horizontal action nonzero - with remaining transition time forcing it zero; pinned by an_active_body_reference_springs_back_after_the_direct_angle. Scope notes: retail ties the consumer to one value of a menu-side control setting this record leaves unmapped (its own gap list), so kuluu reads that predicate active via mouse.rs BODY_REFERENCE_CONTROL_CONFIG_ACTIVE as an authored choice; and follow_share's per-tick blend stays, since it smooths look point and distance - channels this record does not cover - while the new correction rides the angular channel after the manual orbit |
| camera aim height consumes a movement-probe answer's vertical word directly: the unit probes with an id byte from its own accessor (skipping one sentinel value) and adds the second word of that answer straight into a local accumulator feeding aim height - no intermediate global on this path | kuluu/src/view_native/camera_collision.rs resolve_camera | differs: kuluu's camera derives height placement from its own ray resolution and never consumes walker-side probe answers (collision record, 2026-10-25 re-read section) |
| manual yaw input passes a pending accumulator and a ten-frame-tick latch: press cycles completing inside the window net zero view rotation, held presses orbit at the direct rate ("Manual orbit tap dwell") | kuluu-render/src/mouse.rs OrbitKeyDwell with ORBIT_TAP_LATCH_FRAME_TICKS / ORBIT_TAP_DWELL_SECS, consumed by the analog-stick yaw feed in kuluu/src/view_native/input.rs dispatch_movement_system (a gated-off stick flick also stops counting as camera_panning, so it cannot jitter the steer latch); pinned by a_press_held_through_the_latch_opens_on_the_tenth_frame_and_stays_open, a_release_restarts_the_latch and the latch-window constant pin in kuluu-render/src/mouse.rs | matches on the observed law for the analog stick only since 2026-10-28. KEYBOARD EXEMPT BY USER RULING 2026-10-28 ("arrow left and right have a noticible start lag.. arrow keys should not have spring! spring comes from moving the character.. nto the cameral"): an unlocked arrow writes its orbit step the frame it is held, no dwell - the felt cost of retail's latch (~ten dead frames at every press start) was ruled worse than the tap-swallow benefit for keys, while the stick evidence stands untouched. Re-keyed pins: rapid_alternating_camera_key_taps_net_out_with_no_start_lag (first-frame motion + per-pair cancellation) and camera_keys_leave_a_locked_camera_alone (kuluu/src/view_native/input.rs). Mechanism was always a press-cycle gate rather than retail's unresolved pending-accumulator composition; mouse drag never passed through this gate - its button engagement machine already refuses bare clicks (this record, tap-dwell pass; keyboard exemption 2026-10-28) |
| the followed actor's drawn height arrives eased upstream while its committed and wired numbers stay stepped: retail's mover commits through a state block fed by per-frame probe measurement (collision record, "Terrain height and gravity, per frame"), so a stair riser reaches the view as a slope, never as a full-height pop | kuluu-render/src/scene.rs SelfVisualY in interpolate_self_transform_system - drawn Y follows the lerped physics commit at the rig cascade's recorded weights (0.5 ease / 0.25 spring per movement tick), XZ stays exact, warps seed exact, lag bounded to SELF_VISUAL_Y_MAX_LAG_YAMLS; pinned by a_riser_commit_reaches_drawn_height_sloped_not_whole, visual_y_lag_stays_bounded_during_a_long_fall and a_warp_seeds_exact_and_risers_still_slope_afterwards | matches in effect as an authored placement (added 2026-10-27 after the user's jackhammer playtest report: the rig cascade alone could not absorb it because kuluu's own drawn feed delivered the riser whole): retail eases upstream inside the mover; kuluu keeps physics, wire reports and collision on exact logical feet and eases only at the render seam, so no physics behaviour changes. The lag budget is kuluu-authored (a yalm - above every measured shipped single-tread rise, below an unconstrained fall drift); retail's own drawn-feed law upstream of its commit stands as recorded in the collision record (this record, tap-dwell day) |
| the rig is a persisted chase triple eased toward the actor pose by one-half per frame then chased by nought-point-two-five per tick-proportional substep, all three axes; discrete pose jumps arrive attenuated | kuluu/src/view_native/camera_collision.rs ChaseCascade (consts CHASE_EASE_ALPHA / CHASE_SPRING_ALPHA), advanced and consumed in resolve_camera ahead of the focus dead-zone, aim and boom; pinned by chase_cascade_pins_the_recorded_weights_after_a_jump, chase_cascade_attenuates_and_settles_without_overshoot, chase_cascade_substeps_track_elapsed_ticks, chase_cascade_caps_substeps_after_a_hitch and a_stair_jump_reaches_the_camera_attenuated_and_settles | matches (closed 2026-10-27): persisted per-rig eased+sprung triple, one-half ease once per rendered update, nought-point-two-five chase over tick-proportional substeps (elapsed movement ticks from the frame's delta time), exact pose reaching the camera only through the cascade. Authored deviations: the spring share is taken fractionally - one update closes `CHASE_SPRING_ALPHA` raised to the elapsed-movement-ticks power as `1-(1-alpha)^ticks`, which equals retail's whole-iteration count bit-for-bit at tick counts and removes the refresh-rate aliasing kuluu's uncapped framerate exposed (at 144 Hz an integer substep count springs on one frame in ~2.4 and stands still on the rest: USER's "W+D causing a jitter.. like an update pause" and "nameplates jitter on a flat surface while running", 2026-10-28, measured as 0 stalled frames over the fix at 30/60/144 Hz in `chase_cascade_springs_identically_at_any_framerate`); an update longer than `FRAME_STALL_SECS` (the catch-up cap plus one tick) re-seeds the persisted triple on the candidate instead of simulating it, so a hitch - the first menu open being USER's reported instance - stays a hitch rather than becoming camera motion (`chase_cascade_reseeds_on_a_stall_and_simulates_a_long_frame`); snap-to-anchor re-seeds the persisted triple exact rather than clearing it, matching retail's reset-writes-exact rule; both stages ride behind settings.camera_spring - off leaves the pre-2027 exact-follow pivot, and retail's mode gate that skips both stages (this record's unmapped-setting gap) has no kuluu counterpart. Spring *strength* is a deliberate kuluu deviation as of 2026-10-28: `CHASE_SPRING_ALPHA = CHASE_SPRING_ALPHA_RETAIL / 2` (nought-point-one-two-five against retail's nought-point-two-five, the recorded value kept named beside it). USER asked for more spring and reported that raising the share felt like less - "wtf.. you made the spring LESS.. I told you i WANTED MORE" - because a bigger share shortens the eye's catch-up time; halving it doubles how long the trail lasts. Pinned by `chase_cascade_run_spring_is_retail_halved_for_trail` |

## Provenance

All code locations below are RVAs in retail-2026-09, image-baseVA0x10000000. Installed DLL SHA256 f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4. Verified raw unpacked .textSHA256 b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9. Installedpath `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`; rawtext `/private/tmp/emission-retail.text.bin`. No process was executed or modified.

Direct input readsRVA0x1ef4a/0x1ef57 call0x123970 with device0x3f/actions6/7. Dispatch tableRVA0x36d108 containsVA0x10120c70/0x101232e0 foraction6.0x120c70 reads analog centeredbyte-128;0x1232e0 routes keyboard after menu/device checks. Actionreader0x123970 dispatches each action callback; separate active-input masks canzero it. Horizontal reversalconfig139 queried0x1ef67, inversion0x1ef78..0x1ef82; verticalconfig140 after0x1ef86. Actualmanualblock0x1f007..0x1f0ed: parallelpredicatevtable+0x340; frame-tick0x14cf0, constRVA0x32a3ec=.027924444526433945; modevtable+0x330;distance3D subtraction+dot+sqrt0x1f040..0x1f078;floorRVA0x329a18=.009999999776482582;rateRVA0x32a3e8=6; transitiontimerVA0x10456d7c; callpolarapplier0x1ebb0. Applier0x1ebb6..0x1ec56 usesfocus-eyeX/Z, FPATAN atan2(Z,X), horizontalradius,cos/sinrebuild.

Concrete actor vtableVA0x1032d710 installedbyconstructorRVA0xa4450;entry+0x330 atRVA0x32da40 pointsRVA0xa4670, a side-effect-free getter ofactorbyte+0xf9;entry+0x340 pointsRVA0xa46b0, getter ofbyte+0xfb. Community namesIsFreeRun/IsParallelMove are locator labels, not independently verified user-mode semantics.

Reference setterRVA0x1e2f0 writes modeVA0x10456db0/referenceVA0x10456db4 fromarguments;noTTL. Bodyproducer0xa692a..0xa6998 computesheadingterm, addsit tobodyheadingat0xa6940, writesheadingat0xa6988, then passes same term/mode1 tosetter. Steer-zero branches0xa69aa/0xa6a46/0xa6a5d and laterconfiguration reset0xa6c29 passmode0/reference0. Camera reset0x1e685 callssetter with zeros.

Referenceconsumer0x1f14d calls0x25e050, which returns whether a configurationobject+0x44equals2.0x1f15a..0x1f188 requiremodegettertrue, modebyteactive, actionh at[esp+0x1c]nonzero.0x1f18e..0x1f19a negatesreference.0x1f19e repeats the same modegetter andjumps0x1f20a when true, thus bypassing0x1f1a8..0x1f208distance scaleonstablegetter.0x1f218..0x1f235 suppresses when modegettertrue andpositive transitiontimer.0x1f24e..0x1f255 callsorbitapplier. This entire block occurs afterdirectmanualorbit.

Own saveddisassembly `camera-channels.asm`, `camera-consumer.asm`, `camera-facing-producer.asm`; no implementation writer may readthese mechanics. Source inlineRVAs and communityCameraManager/ControllableActor/InputActionDefinitions were locator hypotheses only. Conditional facts above were verified inactualretailbytes. No source edits, implementations, commits or external posts occurred.

Re-verified on the 2026-10-05 build (generated C recompile per vendor/rea/INSTALL.md; provenance
"generated C, build 2026-10-05"), all September addresses unchanged:

- Update unit `f_1001ee60`, covering `0x1001ee60..0x10020b9f`; manual orbit block
  `0x1001ef4a..0x1001f255` with action reads for 6 and 7, frame-tick call at the same site as before,
  transition timer dword `[0x10456d7c]`, polar applier `f_1001ebb0` used twice. Orbit constant near
  site `0x1001fcc8`. Recomp hook sites unchanged (build meta): cam_tick `0x1001fa55`, swing
  `0x1001fcc8`, wait `0x1001fb9c`, cam1..4 `0x1001fd26`/`0x1001fd4c`/`0x1001fda5`/`0x1001fe36`, look1
  `0x1001f5d9`, look2 `0x1002017c`.
- Reference setter `f_1001e2f0` (mode `[0x10456db0]`, active `[0x10456db1]`, reference dword
  `[0x10456db4]`), called six times from the walker; state block `0x10456d70..0x10456db4` with wait latch
  at `0x10456d8c` and both-zoom flag at `0x10456d84`.
- Camera unit receiver getter `f_10015250` = dword at `[[0x104568fc]+0x50]`; the manager global is written
  by init functions `f_10001240`, `f_100013c0`, `f_10001520`.
- Virtual-slot census inside the unit: accessor `[edx+0x1bc]` eighteen times, of which four sites form the
  pull-in cluster `0x10020640..0x100206b4` (reads block floats at `[eax]`, `[eax+4]`, `[eax+8]` and a second
  call's `[eax+0xC]`; subtraction of f32 unit constant `[0x1032961c]`; transform copy through `f_1001e760`
  which does `add ecx,0x44; call f_10026eb0(out,in)`; stack query helpers `f_10026eb0` and `f_10028230` over
  list head `[ebx+0x44]`). Query handler invocations at camera sites `0x1001f344`, `0x1001f4fa`,
  `0x10020044`. Accessor semantics confirmed by the shared impl `f_100a4740` (`lea eax,[ecx+0x5FC]`).
- Engine and record layout behind those calls: see the 2026-10-25 collision record (engine `f_1002c3e0`
  to `f_1002c400`, readiness `f_10071060` with manager dword `[0x1047d128]`).

Leash and spring (established pass of 2026-10-27; "generated C, build 2026-10-05", all sites inside update
unit `f_1001ee60`; frame-slot identities derived mechanically by an esp-depth walk over the recompiled text,
conflict-free across every window cited):

- Ease helper `f_100276a0(out, B, A, t)` computes componentwise `(1-t)*A + t*B` (one-seed constant
  `[0x1032961c]`, body sites `0x100276a0..0x1002771f`). Exactly four call sites exist in the client:
  camera ease `0x1001f526` (in-place into ease triple T; operands out=T, B=candidate U, A=T; alpha immediate
  `push 0x3f000000` at site `0x1001f50f`), two sites in `f_1001a420` (`0x1001a5f4`, `0x1001a609`) on the
  system global pair around `[0x1045b830]/[0x1045f040]`, and one in `f_1002ac60` (`0x1002ad84`).
- Candidate copy: accessor call `[edx+0x1bc]` at site `0x1001f4db`, block copied through `f_10026eb0` at
  site `0x1001f4e7` into the candidate triple (frame slots `-0x1c4/-0x1c0/-0x1bc` relative to entry esp).
- Vertical feed into U's second component: `fld [slot -0x1c0] / fadd [slot -0x18c] / fstp back to [slot -0x1c0]`,
  sites `0x1001f500`, `0x1001f504`, `0x1001f521`.
- Spring loop, one iteration: `call f_10027120` at site `0x1001f5c7` writes the difference triple (frame-slot
  bases `-0x170/-0x16c/-0x168`) = ease triple (base `-0x160`) minus chase triple (base `-0x1a0`); scale
  `call f_100272b0(out, imm)` at site
  `0x1001f5d9` with `push 0x3e800000` (nought-point-two-five) at site `0x1001f5d3`; add `call f_10026f20`
  at site `0x1001f5eb` folds the scaled difference into the chase triple (base `-0x1a0`); loop-back
  `jne 0x1001f5b2`. The recompile hook family on this region matches builds.json (`cam_swing 0x1001fcc8`
  area, look hook `frame_look1 0x1001f5d9` sits directly at the scale call).
- Substep count N: tick-unit framerate getter `f_10014cf0` (reads `[manager+0x28]`, NaN-guarded against
  one-seed `[0x1032961c]`) called at site `0x1001f5a2`, integerized by CRT helper `f_10311c6c` (classic
  x87 `or ah,0xc; fistp` float-to-int) at site `0x1001f5a7`; `test eax,eax / jle skip` sites
  `0x1001f5ac/0x1001f5ae`. The orbit block reads the same persisted pair the cascade maintains: subtraction
  with operands `[unit+0x44]` and `[unit+0x50]` at sites `0x1001f04c..0x1001f05c`, dot/sqrt/`6/max(d,0.01)`
  scale at `0x1001f071..0x1001f09c` (constants `[0x10329a18]`=0.01 floor, `[0x1032a3e8]`=6 - identical to the
  September census).
- Chase triple persistence: loaded by `f_10026eb0(unit+0x50 -> chase local)` through getter `f_10015250` at
  sites `0x1001f587..0x1001f595`; written back by setter `call f_1001e780(unit, &chase)` at site
  `0x1001f602` (its head computes `(arg - [unit+0x44])` length and branches).
- Mode gate: `xor bl,bl` at site `0x1001f530`; followed-actor mode getter call `0x10084390` at site
  `0x1001f532`; compares against `0x2f`, band `0x3e..0x53` at sites `0x1001f537..0x1001f546`; second gate
  producer sets the flag in the anchor-height branch at site `0x1001f56e`; skip target `0x1001f5f6`.
- Method note: textual `[esp+off]` references were normalized by walking push/pop/add/sub deltas over the
  generated C with jump edges (`jcc` hex targets included); adjacent scratch triples at overlapping frame
  slots are compiler reuse of dead temporaries across non-overlapping live ranges, confirmed per-window.

Manual orbit tap dwell (established pass of 2026-10-27; "generated C, build 2026-10-05", all sites inside
update unit `f_1001ee60` unless noted):

- Action reads: device `0x3f` action 6 at site `0x1001ef4a` and action 7 at `0x1001ef57`, both through the
  shared reader `f_10123970` (`call f_10123970` sites `0x1001ef4e`/`0x1001ef5b`), stored to frame scratch.
  Inversion options gate the two slots by multiplying against float `[0x1032a3f0]`, read from this build's
  image as minus one, sites `0x1001ef6c..0x1001efa5` - the horizontal/vertical reversal already recorded for
  retail-2026-9.
- Pending accumulator: after the polar-applier call at site `0x1001f0ed` (second use of `f_1001ebb0`),
  frame-tick call at `0x1001f0f2`, then `fmul` by the action-6 scratch slot (`[esp+0x24]`) at site
  `0x1001f0f7` and by float `[0x1032a3e4] = nought-point-one-oh-six-six-six` (value read from this build's
  unpacked image) at site `0x1001f0ff`; the product compares against nought (`[0x103295d8]`, float +nought)
  in sites `0x1001f12e..0x1001f145`, and on that gate the dword latch write `mov [0x10456d74], ebp` (latch
  value ten, register loaded at site `0x1001f014`) lands. The accumulation itself is sites
  `0x1001f147..0x1001f14a` (`fadd/fstp dword [edi+0x48]`), with a sibling accumulate at site `0x1001f29a`.
- Pending-pair consumers (semantics not resolved this pass): difference-and-sign block over `[edi+0x48]` and
  `[edi+0x54]`, sign selected against float +nought, compared against global `[0x10456d78]`, clamped store
  back into `[edi+0x48]` at sites `0x1001fea4..0x1001fee5`; a second compare-and-store pair over the same
  fields at sites `0x100207b9..0x10020837`.
- Float values read from the unpacked image (pefile memory-mapped reads of
  `generated/images/2026-10-05/FFXiMain.unpacked.dll`): `[0x1032a3e4] = nought-point-one-oh-six-six-six`,
  `[0x1032a3e8] = six` (the recorded nominal radius, unchanged from retail-2026-9), `[0x1032a3ec]` the
  recorded orbit-rate constant (unchanged), `[0x1032a3f0] = minus one`, `[0x103295d8] = plus nought`.
- Grounding for the visible law: user playtest on retail, 2026-10-27 - rapid alternating keyboard left/right
  taps move the followed camera not at all. Kuluu-side implementation and its pins are in the delta row
  ("Manual orbit tap dwell"); kuluu constants live in `kuluu-render/src/mouse.rs`
  (`ORBIT_TAP_LATCH_FRAME_TICKS`, `CAMERA_FRAME_TICK_SECS`, `ORBIT_TAP_DWELL_SECS`, `OrbitKeyDwell`).
