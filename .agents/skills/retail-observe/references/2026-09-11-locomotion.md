# Remote mob locomotion model, 2026-09-11

Observation record for Kuluu's remote-entity chase model: how a roaming or chasing mob's position,
gait and facing are driven off the 0x0E POS block. Wire facts verified against pinned vendor/server
(SHA `c39004cef47fa1bde98c63ccc41874e86145e0af`) first, retail decode second. The MOTION_UPD probe
(`KULUU_MOTION_LOG=1`, tracing target "motion") is the regression guard: it prints band, step,
ratio and both timing halves (segment budget and ring max) on one line per POS update.

## Wire facts

The 0x0E CHAR_NPC POS block (pinned vendor/server `src/map/packets/entity_update.cpp`) carries,
among others: `moving` u16 at 0x18, `speed` u8 at 0x1C (the MovementSpeed2 source),
`animationSpeed` u8 at 0x1D. The two speed bytes are independent by construction:
`CBattleEntity::UpdateSpeed(run)` (`src/map/entities/battleentity.cpp`) multiplies only the
movement speed when running (roam calls it with run=false, chase with run=true and the run
multiplier); `animationSpeed` is never multiplied. The retail decode walks the local position
toward the POS target at `MovementSpeed2 = Speed * 0.1` yps and stops on arrival; there is no
velocity extrapolation.

Y is server-resolved inside `CPathFind::StepTo` (`src/map/ai/helpers/pathfind.cpp`): on arrival the
position snaps exactly to the waypoint, otherwise XZ advances by the step remainder and Y walks
toward the waypoint's height along the slope, clamped between start and end. The client assigns
rendered.y = server_pos.y directly on every update: no smoothing, no gravity. Y is excluded from
the snap-band jump metric so a floor-height change cannot inflate it.

The heading byte rides in the same POS block: `CPathFind::StepTo` calls `LookAt(pos)` and
`CPathFind::LookAt` writes `loc.p.rotation = worldAngle(from, to)`. Heading and position therefore
land on one snapshot; Kuluu applies both from that snapshot (the rendered heading eases toward the
new byte with a short time constant so the body whips rather than teleports).

## Step model

The wire is a discrete step stream: per AI tick a moving mob advances exactly one step and emits
exactly one POS update. `CPathFind::StepTo` computes `stepDistance = speed / (run ? 50 : 40)` yalms
and takes it once per logic tick; the run flag comes from PATHFLAG_RUN, which the mob controller
sets for chase/follow/return-home and not for roaming. So roam is walk (/40) and engaged is run
(/50), and since UpdateSpeed multiplies only `speed`, the split reads straight off the wire as
`speed > speed_base`.

Kuluu's `expected_step_yalms(speed, speed_base)` mirrors this: divisor 50 when `speed >
speed_base`, else 40 (named constants cited to StepTo). StepTo also substitutes a local
`speed = 20` for ROAMFLAG_WORM mobs whose GetSpeed() == 0 and never writes it back, so the wire
byte stays 0 while the worm still moves; Kuluu applies the same substitute when the speed byte is
0 on an update that actually moved. No other literals are used by the step model.

## Cadence ring

The arrival cadence estimate is an 8-sample ring of measured inter-update intervals, newest last:

- Seeded with one AI tick per slot: pinned vendor/server `src/map/map_constants.h`
  `kLogicUpdateRate = 2.5f`, so one tick is 0.4 s (`kLogicUpdateInterval = 1000 / kLogicUpdateRate`).
- On each real position change the measured interval (clamped to half a tick .. two and a half
  ticks, both derived from kLogicUpdateRate) replaces the oldest sample.
- The segment budget is `max(ring) * INTERVAL_HEADROOM`. Max-of-ring is asymmetric on purpose: one
  long gap widens the budget immediately; a burst of fast arrivals cannot shrink it back until that
  sample ages out. An EMA gets this backwards for the stall problem.
- Gaps beyond five ticks (an engineering bound, not LSB-derived) are stale: the ring resets to its
  kLogicUpdateRate seed and the position still tweens. Staleness is a timing event, never a band
  input.

INTERVAL_HEADROOM (1.25x the widest recent interval) and the idle multiplier are named engineering
bounds with comments saying they are not LSB-derived; everything else in this section derives from
kLogicUpdateRate.

## Arrival tween

Per frame the rendered position advances `rendered += (server - rendered) * (dt / remaining)` where
remaining is the segment budget minus elapsed time. The target is reached exactly at budget end by
construction: no early arrival, no overshoot, and a late packet finds the entity holding on target
inside its segment instead of dropping to idle between updates (retail behavior: walk to target,
stop on arrival). There is no separate hold window; the segment is the hold.

## Snap bands

On each POS update, `jump` is the XZ distance between consecutive confirmed server positions (what
LSB actually moved this tick, never where Kuluu happens to be rendering) and `step` is
expected_step_yalms for the incoming bytes. The band is a ratio to the step, decided before the
tween runs:

| Band | Condition | Action |
| --- | --- | --- |
| Normal | jump <= 1.0 * step (+ a small float-boundary epsilon) | chase (tween) |
| Stretch | 1.0*step < jump <= 2.0*step: a path re-eval or a late tick | chase, no snap |
| Pop | jump > 2.0 * step | rendered XZ set to the server position directly |

The band is distance-only: staleness resets the cadence ring but does not pop. A fixed-distance
snap (the old 4 yalms^2 constant and upstream's 20 yalms) read fast mobs' ordinary ticks as
teleports; measured against what LSB actually moved, the ratio is invariant to the mob's speed.

## Gait and playback rate

run = `speed > speed_base`, else walk: one rule for Mob/Pet/Npc/Pc (LSB lifts only `speed` when it
runs an entity, so the comparison is the whole signal; it also picks the step divisor). Self keeps
its input-driven gait. Walk/run clip playback scales by `anim_rate_scale(speed_base)` =
`speed_base * SPEED_TO_YPS / AUTHORED_ANIM_RATE`. SPEED_TO_YPS (0.1) is retail's own decode factor
(research/XiPackets world/server/0x000E: AnimationSpeed = SpeedBase * 0.1); AUTHORED_ANIM_RATE is a
Kuluu inference, the reference rate clips are authored at, derived from that same factor applied to
the base packet speed. A zero animationSpeed byte means "no authored rate" and scales by 1.0: pinned
vendor/server `sql/npc_list.sql` ships NPCs with speedsub = 0 (Resistance_Fighter 100/0), so a zero
must not freeze the walk clip. The scale applies on the locomotion tier only, gated by the model's
0x45 movement byte; Flying and Sliding have no ground stride to match and play at the authored rate.

## Remote grounding

LSB grounds mobs to the Detour navmesh, not the render mesh: waypoints come from Detour (pinned
vendor/server `src/map/navmesh.cpp` CNavMesh::findPath / findRandomPosition over
DetourNavMeshQuery) and StepTo walks Y to that waypoint. Detour poly heights differ from the MZB
collision surface by up to a navmesh cell height, so the POS packet Y is approximate: it picks the
level, and Kuluu places remote ground movers on its own collision mesh every frame at the rendered
(x,z), with server Y as the reference level. This runs after the prediction tween so the model
rides the slope during interpolation; there is no history, no distance test and no snap constant
(the bands own the jump decision). Gate: the 0x45 Info movement byte from the loaded model; Flying
keeps server Y and is never grounded, everything else grounds. ground_nearest returning None
(unloaded interior, off-mesh) falls back to server Y with a once-per-entity debug log. Kuluu
inference from the LSB navmesh model, not an observed retail client behavior.

## Heading byte

Pinned vendor/server `src/common/utils.cpp` `worldAngle(A, B)`: within 0.1 yalms it keeps A's own
rotation; otherwise `byte = atan2(B.z - A.z, B.x - A.x) * -(128 / pi), mod 256`. The heading is
measured from the wire +X axis and negated: byte 0 faces +X. Inverting with theta = byte * 2*pi/256,
the wire forward direction is (cos theta, -sin theta); through Kuluu's ffxi_to_bevy(x, -z, -y) that
is Bevy forward (cos theta, 0, +sin theta), which is what `heading_forward` returns and what the
MOTION_MIS probe measures travel against (`atan2(vz, vx)` in Bevy space). The remote actor's
Transform yaw is `from_rotation_y(-theta)`, consistent with models authored facing +X at byte 0.

## Crit vs recoil

Two independent inputs on the BATTLE2 result block (pinned vendor/server
`src/map/action/action.cpp` `action_result_t::recordDamage`):

- hitDistortion is set purely from damage as a percent of target max HP: >=20 Heavy, >=10 Medium,
  >0 Light. It drives the recoil clip size: Heavy plays `ldam` when the model ships it, else falls
  back to `damg`; Medium and Light play `damg`; None plays no damage reaction.
- The crit is signaled only by `info |= ActionInfo::CriticalHit`. It is not hitDistortion == 3; the
  two fields are independent inputs. No DAT in this install ships a crit-specific routine, so the
  crit bit currently has no visual consumer beyond what recordDamage already routes; that is
  documented at the routing site rather than invented.

## 0x45 Info fields

The model DAT's 0x45 Info record: ffxi-dat parses the first 15 of its 16 body bytes (the sixteenth
stays uninterpreted). Field names follow research/xim `InfoSection.kt` (no line pins):

| Offset | Field | Meaning |
| --- | --- | --- |
| b[0] | movement_type | 0 Walking, 1 Sliding, 2 Large, 3 Flying; out-of-table bytes keep the raw value as Unknown(u8) rather than collapsing to Unset (the 0x08 range fallback is live data). Gates grounding and the stride scale |
| b[1] | footstep_material | movement char (base36) |
| b[2] | footstep_size | shake factor |
| b[3] | motion_index | battle idle pack index; kept raw, used as a DAT offset |
| b[4] | motion_option | weapon sub byte |
| b[5] | is_shield | selects the upper-body motion DAT |
| b[6] | weapon_constrain | uninterpreted |
| b[7] | unknown2 | uninterpreted |
| b[8] | weapon_unknown3 | uninterpreted |
| b[9] | body_armour_waist | selects the waist/skirt motion DAT |
| b[10] | scale | model scale in percent, 0xFF = default; applied to NPC subjects only (PCs drop their scale byte retail-side) |
| b[11] | static_npc_scale | uninterpreted: the retail swap-in for seated/static NPCs is not implemented and the accessor was deleted with its consumer |
| b[12] | unknown7 | uninterpreted |
| b[13] | unknown8 | uninterpreted |
| b[14] | motion_range_index | range type; out-of-table bytes keep the raw value as Unknown(u8) |

## CLIP_WARN reasons

CLIP_WARN (tracing target "clip", on by default, once per world_id/clip/reason) reports a selected
clip or routine that resolved to nothing:

| Reason | Meaning |
| --- | --- |
| not_found | no chunk in the model DAT matches the parameterized id |
| seq_load_error | motion chunk present in the DAT walk but rejected by parse; per-model rejected-chunk list makes this distinguishable from not_found |
| no_match_kept_previous | selected id had zero matches and current_clip was left untouched: the frozen-mob signature |
| not_found_override_skipped | a higher-priority tier (special/fishing) asked for a clip the model does not ship; selection falls through to the next tier |
| routine_not_found | the named routine chunk is absent from the model DAT |
| routine_seq_load_error | the routine chunk is present but its stage stream failed to parse (per-model rejected-routine list, mirroring rejected_clips) |
| routine_no_motion_stage | the routine parsed but carries no Motion stage, so it yields no pose clip |

## Standing note

All speeds and distances in this system derive from wire bytes of the moment or pinned-SHA LSB
constants: the step divisors 50/40 and the worm substitute 20 (StepTo), the tick period
(kLogicUpdateRate), the band multipliers as ratios to one StepTo step, and retail's own decode
factors for clip playback. Nothing here assumes a per-mob value; per-species and per-NPC speeds in
LSB are data, not code.
