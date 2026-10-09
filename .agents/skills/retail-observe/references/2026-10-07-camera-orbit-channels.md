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

## Provenance

All code locations below are RVAs in retail-2026-09, image-baseVA0x10000000. Installed DLL SHA256 f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4. Verified raw unpacked .textSHA256 b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9. Installedpath `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`; rawtext `/private/tmp/emission-retail.text.bin`. No process was executed or modified.

Direct input readsRVA0x1ef4a/0x1ef57 call0x123970 with device0x3f/actions6/7. Dispatch tableRVA0x36d108 containsVA0x10120c70/0x101232e0 foraction6.0x120c70 reads analog centeredbyte-128;0x1232e0 routes keyboard after menu/device checks. Actionreader0x123970 dispatches each action callback; separate active-input masks canzero it. Horizontal reversalconfig139 queried0x1ef67, inversion0x1ef78..0x1ef82; verticalconfig140 after0x1ef86. Actualmanualblock0x1f007..0x1f0ed: parallelpredicatevtable+0x340; frame-tick0x14cf0, constRVA0x32a3ec=.027924444526433945; modevtable+0x330;distance3D subtraction+dot+sqrt0x1f040..0x1f078;floorRVA0x329a18=.009999999776482582;rateRVA0x32a3e8=6; transitiontimerVA0x10456d7c; callpolarapplier0x1ebb0. Applier0x1ebb6..0x1ec56 usesfocus-eyeX/Z, FPATAN atan2(Z,X), horizontalradius,cos/sinrebuild.

Concrete actor vtableVA0x1032d710 installedbyconstructorRVA0xa4450;entry+0x330 atRVA0x32da40 pointsRVA0xa4670, a side-effect-free getter ofactorbyte+0xf9;entry+0x340 pointsRVA0xa46b0, getter ofbyte+0xfb. Community namesIsFreeRun/IsParallelMove are locator labels, not independently verified user-mode semantics.

Reference setterRVA0x1e2f0 writes modeVA0x10456db0/referenceVA0x10456db4 fromarguments;noTTL. Bodyproducer0xa692a..0xa6998 computesheadingterm, addsit tobodyheadingat0xa6940, writesheadingat0xa6988, then passes same term/mode1 tosetter. Steer-zero branches0xa69aa/0xa6a46/0xa6a5d and laterconfiguration reset0xa6c29 passmode0/reference0. Camera reset0x1e685 callssetter with zeros.

Referenceconsumer0x1f14d calls0x25e050, which returns whether a configurationobject+0x44equals2.0x1f15a..0x1f188 requiremodegettertrue, modebyteactive, actionh at[esp+0x1c]nonzero.0x1f18e..0x1f19a negatesreference.0x1f19e repeats the same modegetter andjumps0x1f20a when true, thus bypassing0x1f1a8..0x1f208distance scaleonstablegetter.0x1f218..0x1f235 suppresses when modegettertrue andpositive transitiontimer.0x1f24e..0x1f255 callsorbitapplier. This entire block occurs afterdirectmanualorbit.

Own saveddisassembly `camera-channels.asm`, `camera-consumer.asm`, `camera-facing-producer.asm`; no implementation writer may readthese mechanics. Source inlineRVAs and communityCameraManager/ControllableActor/InputActionDefinitions were locator hypotheses only. Conditional facts above were verified inactualretailbytes. No source edits, implementations, commits or external posts occurred.
