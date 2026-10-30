# Cutscene scheduler stage tags and the event emote trio — 2026-10-28

## Method and scope

Static only. Everything here came from reading this install's own DATs and from the generated C of
the current client build (see Provenance). No live session, no retail observation, nothing executed.

DAT inventory reproduces with `cargo run --release --example dat-routine-stages <file_id> ""`; that
example now also prints the payload dword and word count for stages kuluu cannot classify, which is
how the payloads below were measured.

## The cutscene carriers behind the chocobo-rental scenes

| file_id | path in the install | routines | role observed in the live cue streams |
| --- | --- | --- | --- |
| 30904 | `ROM/62/110.DAT` | 56 | the screen-colour fades both zones use (`fdo0..2`, `fdi0`), plus camera-route carriers and the other staging families named below |
| 30905 | `ROM/62/111.DAT` | 9 | the rental NPC's own routines: its sound routine, and six white-screen drives (`who0/1/2/3`, `whi0/1`) |
| 30906 | `ROM/62/112.DAT` | 12 | the Bastok Mines local-player camera routes; every stage in the file is a camera route |
| 30834 | `ROM/62/82.DAT` | — | Southern San d'Oria's local staging for the same scene beat |
| 30907 | `ROM/94/118.DAT` | — | Southern San d'Oria's other local staging routine |

Both rental scenes produce structurally identical cue streams (18–19 cues, same tags in the same
order: fade-out, camera lock, look-at, the kneel motion, a player camera route, the sit motion, the
NPC sound routine, hide, mount), recorded in `artifacts/verify/bastok_chocobo_rental_summary.txt`
and `artifacts/verify/chocobo_rental_summary.txt`. The only zone-specific difference at cue level is
which carrier supplies the player's three camera-route routines: Bastok takes all three from 30906,
San d'Oria takes two of them from 30834 and 30907. So a difference in how those two scenes look to
the player cannot originate in the wire layer or in which cues kuluu emits — it is either inside a
stage kuluu does not execute, or inside the actor (its model and motion package), or inside how an
attached camera route is applied.

## Screen-colour staging: kuluu drives one register of three

Kuluu classifies exactly one screen-colour stage tag (`ScreenColorDrive`, driven through
`kuluu-render/src/cutscene.rs`) and renders that as the whole-screen fade. Measured in the carriers
above, the corpus uses at least four further full-screen staging stages:

| tag | word count | measured payload (the stage's destination bytes) | routines seen | what the closest verified source says it does |
| --- | --- | --- | --- | --- |
| display-colour drive | 3 | `[00,00,00,80]`, `[FF,FF,FF,80]`, `[80,80,80,80]` — the same byte scale with neutral at 128 that kuluu already reads for its own fade destination | `fdol`/`fdil` (300 frames), `fao1/fao2`, `fai1/fai2` (60 and 120 frames), `fdos`/`fdis` (60) in 30904 | drives a second full-screen colour register of its own, destination from those four bytes, length = authored frames scaled by the scheduler's speed ratio, starting from wherever that register currently sits |
| flash-colour drive | 3 | `[FF,FF,FF,80]` for the `who*` routines, `[00,00,00,80]` for `whi*`; authored lengths 15/60/120/180 frames in 30905 | `who0/1/2/3`, `whi0/1` | drives a third full-screen colour register (the one used for flashes), same task shape as the display drive |
| full-screen render-target pass | 2 | payload all zero; authored lengths 60 and 120 frames | `ovl1`, `ovl2` in 30904 | sets up a half-resolution render target pair, switches the game's texture-display state on for its length — i.e. an extra full-screen composite pass, not a tint |
| colour-and-count stage | 5 | neutral RGB triples with a fourth byte that reads like a frame count (`[80,80,80,2D]`, `[80,80,80,1E]`, `[80,80,80,28]`, `[A0,A0,A0,30]`) and one zero-count follow-up | `blof`/`blon`, `ato0/ati1`, the two `*dkn` routines in 30904 (eleven sites) | unknown — see Not established |

So a scene that stages its fades with display/flash registers gets no kuluu *rendering* for them.
Both raw types classify now — `StageKind::DisplayColorDrive` and `FlashColorDrive`, landed later the
same day in 2026-10-28-event-opcode-no-skip-pass.md, whose stage-dispatch reading (both kinds
allocate the fade task's own task class, tagged by stage type) is what promoted them past a
measured-payload guess — and classification is where kuluu stops: no tick, no composite. Nothing
in the two rental scenes dispatches them, however — they live in shared carriers alongside the fade
routines kuluu does run — which is why they cannot be the Bastok-only difference on their own.

## Event emote trio: what retail actually does

Until this record's writer pass (same day — see "Implementing the trio" below) kuluu skipped all
three of these opcodes by their size (`OP_EMOT`, `OP_PLAYANIM`, `OP_ANIMWAIT` shared one arm in
`ffxi-event/src/vm.rs`). Re-reading the handlers on this build, the behaviour they implement is:

- **Event animation (the opcode kuluu names play-animation).** Targeted at the cutscene's own owner
  actor. If that actor is not present the instruction is passed over. If the actor already carries a
  running action, execution parks on the instruction and re-runs it later — retail does not drop it
  and does not overwrite the running action. When the actor is free, the opcode stores its operand as
  the actor's action parameter, marks the actor as running an event-played animation, and moves on.
  That "event-played animation" mark value is written nowhere else in the whole client image: this
  opcode owns it.
- **Emote.** Targeted at a looked-up actor. An unresolvable or absent actor, or one that retail does
  not consider displayable, gets passed over silently. A busy actor makes execution park and retry. A
  free actor receives its emote id plus a one-byte argument through the same routine the rest of the
  client uses to make an actor emote.
- **Wait for action.** Same lookup and eligibility gate — passed over when the actor is not
  available. Otherwise it parks while that actor's action mark is set and advances as soon as the
  mark clears. It carries no duration: the wait is purely on the actor's action state.

The consequence at reading time: kuluu could not implement any of the three faithfully until it had
a per-actor "carrying an action" state and the same retry-until-accepted behaviour. Both halves are
on record — the retry rule above, and the full lifecycle in the next section — which is why this pass
stopped at recording rather than shipping half of it: a park whose release kuluu had not yet
reproduced would freeze scenes instead of pacing them, exactly the failure
`2026-10-02-event-emote-busy.md` backed out of. With the lifecycle and its idle store read (third
pass below), the trio shipped; what is still open to a full retail match is named in Not established.

## Event animation lifecycle, end to end (this is the thing `wait for action` waits on)

Reading further today: retail does not treat an event-played animation as a fire-and-forget. The
mark that the play-animation opcode leaves behind advances through a fixed sequence of states, one
step per frame of the actor's update, and each step has its own gate:

1. **Requested.** The play-animation opcode stores its operand on the actor and sets the mark to
   "event animation pending". Nothing is played yet.
2. **Waiting for the model.** While the mark sits there, the per-frame state machine checks whether
   the actor's visual half is ready (it reports not-ready whenever that half already has an
   outstanding animation request). Not ready ⇒ nothing happens this frame, the same state runs
   again next frame — so a cutscene that asks for an animation on a still-streaming model simply
   waits instead of dropping it.
3. **Animation requested.** Once ready, retail sets a flag on the actor, hands the stored operand
   over to the visual half as its animation request together with a request-mode marker, and moves
   the mark to "animation started". The operand is an id below 64 and the hand-off is gated by the
   actor's class: several classes accept it, others fall through without any animation.
4. **Routine handed over.** In the next state retail clears that flag, and if the visual half has no
   routine of its own running yet it hands it the authored four-character routine `emot` (with the
   actor as its argument) and advances to a counting state.
5. **Counting.** The counting state increments a per-frame counter on the actor while the named routine
   is still reported as running, and on completion resets three of the visual half's routine slots plus
   the byte beside the mark.

(Pinning the store that returns this chain to "no action" was what remained at writing time; the
third pass below closes it.)

So retail's pacing rule for staging is: *the scene waits because the actor's animation state says
so*, and an event-played animation cannot be lost to a busy model or to streaming — it is retried
until the visual half accepts it. That was what kuluu bypassed entirely by skipping these three
opcodes; the writer pass below restores the staging and pacing halves.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| event animation on the cutscene owner, retried while that owner is busy, then driven through its own state sequence until release | `ffxi-event/src/vm.rs` `OP_PLAYANIM` | matches for the staging (owner = running scene via `attach_scene`; no scene → passed over; busy gate parks without replacing; id occupies the action slot through the pass via `pending_action_starts`/`NUMERIC_ACTION_KEY`) — content deliberately not played: the id→animation tables are named below, unread |
| numeric event animations reaching an actor-motion resource (the visual half's animation-request slot, and the id→index tables behind it — addresses in Provenance) | none — kuluu addresses authored motions by four-character key only | missing, now precisely named: the three per-class tables live in `FFXiMain.dll` image data (extracted here, described below); their index space is not connected to any DAT structure either side has decoded |
| the actor's animation-request slot that an event animation lands in (an id plus a request-mode marker on the visual half) | none: kuluu addresses authored motions by four-character key (`ActorMotion` cue keys such as `kue0`/`sit0`) and has no numeric-id request slot to set | missing — retail's operand is a numeric id under 64, gated by actor class; mapping that id onto kuluu's motion packages is the next read (the per-class acceptance table is named in Provenance) |
| an event animation asked for while the model is not ready is retried, never dropped | none — no equivalent gate; kuluu's motions are applied when their cue lands | missing |
| emote id and argument delivered to a looked-up actor, eligibility-gated, retried while busy | `ffxi-event/src/vm.rs` `OP_EMOT` → `EventCue::Emote`, routed by kuluu-session to its live emote pipeline (`dispatch_entity_emoted`) | matches — lookup/eligibility pass-over, busy gate parks via the per-actor predicate without replacing the running action, otherwise the id+argument cue is emitted; ids with no decoded motion end inert exactly like retail's refusal paths below |
| wait for an actor's action to clear, with no duration of its own | `ffxi-event/src/vm.rs` `OP_ANIMWAIT` (predicate `action_running_any`: holds, pending holds, same-pass starts — per-actor and key-free, exactly like retail's single action-mark byte) | matches — and now safe by construction: every hold the wait can park on has a renderer report or a session deadline sweep behind it, so no armed-but-unreleased state exists; an id kuluu cannot time simply does not arm (retail's own refusal path below makes this exact for the rental scenes' operands) |
| display-colour register drive | `ffxi-dat/src/scheduler.rs` `DISPLAY_COLOR_DRIVE_OPCODE` (raw 0x51) → `StageKind::DisplayColorDrive`, destination carried on `SchedulerStage::screen_color` through the widened colour-drive filter | parse matches (promoted past unknown-stage skip; same-task-class evidence in 2026-10-28-event-opcode-no-skip-pass.md) — rendering missing, still blocked on retail's composite order for the three colour registers |
| flash-colour register drive | same file: `FLASH_COLOR_DRIVE_OPCODE` (raw 0x72) → `StageKind::FlashColorDrive`, same payload path | parse matches — rendering missing, same blocker |
| extra full-screen render-target pass | none: same unknown-stage path | missing — needs a renderer feature (a second composite pass), not merely a new stage kind |
| colour-and-count stage used by the blend on/off and attach-staging routines | none: same unknown-stage path | unresolved in every source available here, so there is nothing to implement yet |
| single screen-colour register with latch at destination | `kuluu-render/src/cutscene.rs` `FadeProgram`, parsed as `StageKind::ScreenColorDrive` | matches for the one register kuluu drives; how retail orders the three registers against each other is not established |
| a cutscene camera route resolves its chunk by the stage's own id, inside the file that was dispatched | `kuluu-render/src/scheduler_runtime.rs` `start_cutscene_camera_tasks` | matches — checked deliberately because 30904 and 30906 ship routines with identical names, so a per-file resolution rules out Bastok picking up San d'Oria's route |
| attached camera routes ride the cue actor's transform; unattached ones play in world space | `kuluu-render/src/cutscene_camera.rs` `attach_matrix`, `live_attach_matrix` | matches kuluu's own sources but is not re-verified against retail frames here — the Bastok scene's three routes are exactly this shape, so this is where a camera-only visual report would point first |
| look-at/facing opcodes aimed at the local player turn that player to face the authored target | was: cue pushed but dropped for self (renderer `moved()` filter, no VM fallback); now: `ffxi-event/src/vm/scene.rs` `turn_scene_player_toward`, applied on `OP_LOOKAT` case 0 and `OP_LOOK_AND_TALK` when the first actor is this scene's local player | matches as of this pass — heading computed with the same formula as walk-leg bearings, published like `OP_SET_FACING` (shared cell + `SceneAction::PlayerPosition`, so it flows to snapshot heading, `self_visual_yaw_system`'s smooth turn, and c2s POS) |
| look-at/facing cues aimed at NPCs apply on those NPCs | `kuluu-render/src/scheduler_runtime.rs` `apply_cutscene_actor_cues` | matches — the `moved()` filter drops only local-player-directed cues; NPC-directed `ActorLookAt` was always applied |
| a departing mount plays its authored get-off (`coff`) and is handed back to the sweep when the routine retires | `kuluu-render/src/scene.rs` `drive_mount_departure_system` | matches as of 2026-10-29 (was stranded: the done-test read "component gone" as "not over", so retirement never released the sweep exemption — the reported stationary chocobo; section below) |
| skeleton locator points are consumed with model-frame negative y = up | `kuluu-render/src/cutscene_camera.rs` `eid_model_point` height-fraction fallback | matches as of 2026-10-29 (fallback previously returned positive-y offsets, which the attach flip landed below the feet whenever the model was not loaded; ground truth from shipped EIDs via `ffxi-dat/examples/zz-eid-refs.rs`) |
| mid-scene actor poses follow the event script's authored order: one approach variant, its continuation leg-thread, then the finale thread that carries the second snap, `c01i`, `c00i` and the fade-out #2 in one program | `ffxi-event/src/vm/scene.rs` scene-latched MOVE arms + REQSET tag threads | **matches as of 2026-10-29 (run5)** — every armed leg, snap and stage lands in DAT byte order; run4's "player ~9 yalms ahead" is retracted with its reason (beat graph below). Composition while he walks stays open |
| an attached camera route whose `InterpFactor` is 0 keeps its seed matrix for the whole shot | `kuluu-render/src/cutscene_camera.rs` `advance`, `lerp_matrix`, `a_zero_interp_freezes_the_start_matrix` | matches — verified against `research/XIClient/src/XIClient/source/World/Camera/CameraTask.cpp` lines 39-41 (seed) and 171-181 (`v3 = InterpFactor / 255`; the per-element blend runs only when it is non-zero). So Bastok's `c*i` routes (interp 0) are authored NOT to follow the actor: framing depends on where the actor stands at seed time |
| shot length vs. walking-leg length in those unfollowing shots | none — route durations come from their carriers, leg times from the VM speed refs, nothing reconciles them | unresolved — run5: each of `c05i`/`c01i`/`c00i` runs its authored 60 frames (1 s) while the legs it films take ~3 s at the authored speeds; after a route expires kuluu holds its last frame, so beats between cuts film ground (frames `run5_rental_cs_dense/007`, `008`: identical framing, no actors). Whether retail's scheduler stretches these routes is not established |
| the mid-event mount dispatch renders the rented chocobo (and its rider) | `kuluu-render/src/ffxi_actor_render.rs` `load_mount_race` | missing — run5 arms the mount at `:43.640` (`actor dispatch (mount): rider=5 mount=Chocobo Yellow`) with WARN `load_mount_race race=32: body files resolved but unrendered [55325]`; every shot after that edge frames nobody, player and mount alike. Undiagnosed: what file 55325 is in this install and which of race 32's body slots resolves to it |

## Rental-scene authored values reach kuluu intact, and the self-facing gap (2026-10-28 second pass)

Re-extracted from this session's live artifacts (`artifacts/verify/{bastok_}chocobo_rental_events.jsonl`)
against the authored operands of both zone event DATs (Bastok `ROM/21/43.DAT`, San d'Oria
`ROM/21/39`; dumped with `cargo run --release --example zz-cs-threads 234 7FFFFFF0 raw-from:3173`).
Every staged value lands in kuluu's stream to the printed precision:

- Bastok `37` snap at t+15.25 → x 43.855 / z −108.883 / y-wire 0.750 / heading 1135 of 4096 units →
  wire byte 70, observed exactly in the snapshot track.
- Both walk legs' goals (refs from `152`) match the live position tracks to three decimals; each leg
  start jumps y to its authored value (e.g. 1.374).
- Terminal `1E` look-at at t+16.75 in both zones, tail-thread teleport Δ(1.69, −5.15) at `c01i`,
  then legs heading 4/33 and the gallop along −z after `c00i` (mount speed ≈8 yps).

So the "angles, actions" complaint cannot originate in staging — it is applied-side.

**The defect found: self-directed look-at went nowhere.** `apply_cutscene_actor_cues`
deliberately drops all ActorMove/Place/Face/LookAt cues aimed at the local player (the renderer must
not fight the walker), but nothing else ever applied those facings — `self_visual_yaw_system` only
follows snapshot heading, and look-at never wrote heading. The terminal "look at the chocobo" of both
rental scenes was silently discarded; the player keeps facing his last walk-bearing through the
conversation beats. San d'Oria hides it because its authored bearings happen to point back toward
the pen; Bastok's geometry exposes it. Fixed in the VM layer (`turn_scene_player_toward`, delta row
above): pinned by three tests —
`look_and_talk_in_a_zone_player_block_faces_the_player_at_the_target`,
`lookat_on_the_local_player_selector_faces_the_tracked_position`, and
`look_and_talk_in_an_npc_block_stays_a_cue` (no leak into NPC blocks) in `ffxi-event/src/vm.rs`.

**Attach-matrix theory chased and retracted.** A suspected double-transform of skeleton locator
points (`eid_model_point` + `attach_matrix`, ~150-yalm misplacement) was disproved: the WGSL header
in `kuluu-render/src/skinned_ffxi.wgsl` applies the instance matrix on top of joint blend, and the
pose-chain comment in ffxi_actor_render.rs says world_pose is actor-local — so model-frame points are
correct as passed. Original code restored exactly; guarded going forward by the invariant test
`every_attach_branch_lands_the_matrix_on_the_actor` (both skeleton and fallback branches must land
the matrix within 2.0 of entity translation). Attached-route starts now log at INFO (`cutscene camera
route started`: routine, camera, total_frames, caster, locator, interp) so a remaining "camera frames
wrong side" report is answerable from the console alone.

## Implementing the trio: refusal gates, the chain's tail, and what shipped (same day)

Before writing any of it I closed three gaps statically.

**1. The emote entry refuses ids by argument, not just class.** Retail's shared emote-entry routine —
where every eligible 0x6E lands, fed the id (low byte) and a sign-extended one-byte argument
extracted from its payload word — returns "refused" as its very first act whenever **the argument
byte is 0x80 or greater**, and its caller then skips the actor mark entirely (addresses and
instruction in Provenance). The Bastok rental thread's three emote operands are `0x80F6` / `0x80FD`
/ `0x8106` (dump: `cargo run -q --release -p ffxi-event --example zz-cs-threads 234 7FFFFFF0`,
lines ~4894/5086/15644; San d'Oria's rental thread contains **no** 0x6E/0x99 at all — the asymmetry
is authored in the scripts). So those specific beats are refused in retail itself: no motion, no
action byte, `OP_ANIMWAIT` immediately falls through. Kuluu's implementation reaches exactly that outcome (cue
emitted; `dispatch_entity_emoted` drops what it cannot resolve — ffxi-vocab's empirical table names
ids below ~96 only — and nothing arms a hold, so the following 0x99 passes). "Wrong angles" for
Bastok therefore does **not** come from these three beats; what Bastok gains from this pass is that
any *in-table* emote id in any scene now plays and paces instead of being skipped by size.

**2. The numeric-id request tables are DLL-baked indexes.** 0x63 stores its operand (must be < 64,
class-gated) as the visual half's animation request: the id itself plus a fixed request-mode marker
of 3 (slots and gating routine in Provenance). Three per-class emote-family id→index tables, read
out of the current install image (contents verified byte-for-byte against the unpacked DLL —
exact values and addresses in Provenance; every unfilled entry reads as no-entry):
- a sparse table covering gesture ids 4–9 plus one pose id and one high id, mapping them into a
  high index range (232–239);
- a second table covering the same ids into a second, denser index range (64–71);
- a dense table spanning many more ids ({0,1,5..24,26..40,48,56,80..88,125..127,256..278}), read
  with special-value tests and a stride-of-8 record step — i.e. a per-entry record, not a fourcc.
Nothing in the DATs (scheduler carriers included — stage keys are fourccs like `ato0`/`s007`) or in
any readable source names those index spaces: closed here as "the numeric-animation content lives
behind three DLL tables with no known DAT counterpart", which retires the earlier hope of mapping
ids onto kuluu's motion packages cheaply.

**3. The chain's tail is inside retail's actor action-state machine after all** (the same machine
whose earlier read stopped at the counting state — dispatch tables and arm addresses in Provenance).
Extracting both dispatch stages from the current install image, the two arms that end a chain are:
- **release-prep state:** if the visual half is not ready, exit unchanged; while the authored
  four-character routine `wait` is still running on it, exit; otherwise start the authored routine
  `res0` on the visual half and move to the next release state.
- **idle state:** two guard tests that exit unchanged, then start `res2` on the visual half and
  **store "no action" into the actor's action mark** — the idle store this record had been calling
  missing.
So a finished chain runs a `wait`/`res0`/`res2` release sequence on the visual half before returning
to idle; which states hand into the release-prep state from counting chains is still unwalked
(unchanged openness, narrowed — Not established item 6), but the *store itself* is no longer
unknown. A third zero store sits in one of the machine's later arms and was not needed for anything
above.

**What shipped from this record** (writer pass, same day; pinned by tests in `ffxi-event/src/vm.rs`):
`OP_EMOT`/`OP_PLAYANIM`/`OP_ANIMWAIT` implemented with retail's eligibility pass-over and busy
gates (`action_running_any`, the per-actor predicate the census demands), emote cues riding
kuluu-session's existing live-emote pipeline, PLAYANIM staged-not-played (table finding 2 above is
the reason stated at the symbol), and the same-pass numeric action mark whose release semantics are
deliberately bounded by kuluu's hold-report/deadline machinery — no new hang surface. Tests:
`emoted_arms_the_animation_yield_for_the_pass`, `emote_trio_passes_over_unresolvable_actors`,
`emote_on_free_actor_emits_the_cue_and_occupies_the_action_slot`,
`emote_parks_while_its_actor_carries_an_action`, `animwait_parks_on_the_actor_and_only_the_actor`,
`rental_emote_then_animwait_spans_one_pass`; the old
`unsupported_event_emotes_preserve_baseline_skip` baseline-skip guard is replaced by these (it
protected exactly the busy-gate behaviour now implemented for real).

## Dismount get-off retirement fix, EID height ground truth, and run4's reproduction of the Bastok framing (2026-10-29)

**The stationary-chocobo bug was a retired-query inversion.** `drive_mount_departure_system` held each
departing mount (`MountDeparting`, sweep exemption while `!done`) until its get-off finished, but the
done-test demanded that the scheduler query succeed: with the old
`scheds.get(e).is_ok_and(|s| !s.is_running(coff))`, the moment `tick_active_schedulers` stripped
`(ActiveSchedulers, ActionAssets, ActionTarget)` together with the entry's post-finish TTL, the query
failed, `is_ok_and` read false, and `done` never set — so the entity outlived its own protection as a
stationary mount next to the rider (the reported ~30 s persist). Absence now counts as finished
(`get_off_over = match scheds.get { Ok(s) => !s.is_running(..), Err(_) => true }`), and the
`MOUNT_DEPARTURE_MAX_FRAMES` (300-frame) ceiling applies from that edge too. Pinned by two regression
tests in `kuluu-render/src/scene.rs`: `a_departing_mount_is_handed_back_when_its_get_off_retired`
(component stripped → done set on the same tick, marker itself surviving for the sweep to see) and
`a_wedged_get_off_hits_the_departure_frame_cap` (entry still running past the ceiling → released
anyway). Live verification run4 (`artifacts/verify/drive_1029/run4_client.log`, hidden window):
wire edge `:25:16.034816 mount departed the wire` → same-ms `get-off routine enqueued stages=6
end_frame=174` (authored `coff`: buck + two gallop loops) → `:25:20.934 get-off ran out; the sweep
takes it back elapsed_frames=295 scheds_present=false capped=false` — retirement via the fixed path,
not the cap. Frames `run4_dismount/` show the riderless chocobo still present mid-burst and gone by
+7.5 s; `run4_dismount_tail/` shows no zombie 18 s after /dismount.

**EID model-frame height ground truth.** `ffxi-dat/examples/zz-eid-refs.rs` (added this pass) prints
named EID points for shipped race skeletons: human race 1 NAME −2.0, BODY_CENTER id 21 at −1.24,
feet ≈ −0.06; chocobo race 32 body centre −1.27 — model frame negative y is up. The `eid_model_point`
height-fraction fallback returned positive y, which after `attach_matrix`'s mzb flip landed below the
feet whenever the actor's model was not loaded (mid-cutscene seeds); fixed with the negation and
pinned by `the_eid_fallback_reads_the_named_points_off_the_actor_height` and
`an_attached_route_tracks_the_actor_as_it_moves` in `cutscene_camera.rs`.

**run4 reproduces the post-fadeout framing deterministically, and re-cites what the carrier files do
NOT contain.** Same stand point (Bastok fixture snapped to the authored 37 teleport), new hidden
drive (`drive_run4.py`, artifacts `run4_rental_cs/`, `run4_npc_check/`). Facts:
- The three camera carriers and the fades carry **no visibility or motion stages at all**
  (`cargo run -q -p ffxi-dat --release --example dat-routine-stages -- 30906 c0 | 30904 fd | 30905 ch`):
  every `c*i` routine is exactly one `CameraRoute op=0x04 dur=60` + terminator; every `fd*` is one
  `ScreenColorDrive` (30/60/120 frames). **`chco` — read in earlier passes as the rental NPC's motion —
  is a single `SoundNonPositional op=0x60 id=8225` sound cue; the "NPC gallop routine" reading of it
  is retracted.** Composition mid-scene therefore lives entirely in event-script actor cues + camera
  attach seeds, not inside these carriers.
- Beat pacing holds on authored lengths: every `WAIT*` armed from its DAT unit count released exactly
  then (c05i/c01i/c00i 60 u → age_secs=1.0; the chco+fdo0 beat → 0.7 s), each with the existing WARN
  "motion hold released on its deadline without a renderer finish report" — kuluu's pacing is the
  authored-length fallback, and no renderer→session motion-finish handshake ever fires in this scene.
- Exactly one scripted `ActorHide` (id 17735727) lands mid-fadeout (`:26.46`, inside the fdo0 beat,
  before `c01i` at :27.18), and no show cue follows until `release_cutscene_actors` strips markers at
  CutsceneEnded — matching the script (hide is authored; nothing re-shows it). Lifecycle healthy: the
  entity never despawns (`cutscene ended; releasing its hidden actors {17735727}`).
- Mid-event wire edge at `:26.48`: the server mounts Csrent on the rented chocobo *during* the scene
  ("actor dispatch (pc)" + "actor dispatch (mount): rider=5 mount=Chocobo Yellow"), with WARN
  `load_mount_race race=32: body files resolved but unrendered [55325]` — reproduced, undiagnosed;
  a candidate for the gallop subject reading as absent in later beats.
- Pose lead: the `c00i` attach seed logs caster transform (53.193, −1.374, 118.507) at :30.06 — ~9
  yalms along the authored exit path — while the authored order is teleport Δ(1.69,−5.15) *at* `c01i`
  and legs/gallop *after* `c00i` (second-pass section). The run3 empty-composition frames reproduce
  inside the `c01i`→fdo2 window with exactly that lead: the attached seed points where nothing stands.
- Pre-fade beats are healthy: dialog-frame captures show Azette named and centred, two Chocobo actors
  flanking, third at frame edge — cast present and framed until the fadeout (frames `run4_rental_cs/`
  shots before +15.6 s relative to base shot).

What settles the remaining framing question stays what this record named before run4: one headless
capture with camera eye/target and every actor's transform stamped at the recorded cue times — the
route INFO (`cutscene camera route started`), attach-seed DEBUG, and cue-length DEBUG lines from
this pass support that capture directly.

## The Bastok rental beat graph, read out of the DAT (2026-10-29 third pass)

`ffxi-event/examples/zz-cs-threads.rs` gained a `walk-all` mode. It was needed: the old listing skipped every
`EVENT_ID_PLACEHOLDER` entry, and **the entire player-side scene lives in placeholder threads**, so no earlier
dump could show it (`cargo run -q --release -p ffxi-event --example zz-cs-threads -- 234 7FFFFFF0 walk-all`).

Zone 234's `ROM/21/43.DAT`, block actor `0x7FFFFFF0`, the threads a REQSET tag indexes:

| tag | offset | shape (operands verbatim from the dump) |
| --- | --- | --- |
| 17 | 3173 | SPEED(ref `0x8000`) → `0x37` snap (refs `0x8093..96`) → stage **`c05i`** (@3185, actor1=actor2=`0x7FFFFFF0`, carrier file 32919) → MOVE case 0 @3202 + case 1 hold @3210 → MOVE case 0 @3212 + hold @3220 → `0x1E` LOOK_AND_TALK at `0x010EA02F` → END |
| 18 | 3228 | SPEED + one leg + hold (no snap, no camera) — the continuation of variant 1 |
| 19 / 21 | 3242 / 3311 | same shape as tag 17 with different authored snap refs, LOOK_AND_TALK at `0x010EA030` / `0x010EA031`; tags 20 @3297 and 22 @3366 are their single-leg continuations |
| 23 | 3380 | **the finale, one program**: SPEED(ref `0x80B2`) → snap → stage **`c01i`** (@3392) → leg+hold @3409/3417 → leg+hold @3419/3427 → SPEED(ref `0x808F`, the gallop) → stages **`c00i`** (@3432) and **`fdo2`** fade-out #2 (@3449, actor=`0x7FFFFFF8`) → final leg+hold @3466/3474 |

So tags 17/19/21 are three *alternative* approach beats (one per stable NPC: entity ids 17735727-28-29, which the
client dispatched as modelid 86/87/86), each ending by turning the player toward that NPC; only one runs. Tag 23 is
the last beat and carries both remaining camera routes plus fade-out #2.

**Ordering fidelity: confirmed, previous suspicion retracted.** run5's instrumented arms (`artifacts/verify/drive_1029/run5_client.log`,
helper `INFO` lines added in the 2026-10-29 second pass) reproduce that byte order exactly:

```
:41.524 snap(0x37) → (43.855, −108.883) h=1135    tag 17 @3176
:41.524 MOVE goal=(43907,−110038) s=13             tag 17 @3202   (stage c05i armed :41.525, route started :41.55)
:42.424 MOVE goal=(44356,−110571) s=13             tag 17 @3212
:43.025 WAIT chco(40u)+fdo0(30u)                   the fade beat
:43.124 MOVE goal=(44946,−110829) s=13             tag 18 @3231 (continuation leg)
:44.324 snap → (46.635, −115.974) h=2              tag 23 @3383
:44.324 MOVE goal=(51347,−116507) s=27             tag 23 @3409   (stage c01i → route started :44.34)
:46.125 MOVE goal=(53193,−118507) s=27             tag 23 @3419
:47.224 MOVE goal=(53193,−134363) s=80             tag 23 @3466   (stage c00i + fdo2 → route started :47.23)
```

Six legs total = the chosen approach variant's two, its continuation one, and tag 23's three — not three variants
run back to back. The player walking before the first fade-out is what the DAT authors (`c05i` route + walk, then the
NPC script dispatches `fdo0`; see also block `0x010EA032`, whose script REQSETs tag `0x11` on the player stack *before*
it dispatches `fdo0`), so "the player should stand until after fade-out" is not a kuluu deviation to fix: it needs
retail pixels or a different event variant before anything moves.

**Where the empty shots come from.** Two things, both measured this pass:
- The three routes are attached (`attach=337`, mode 1 + locator 21 BODY_CENTER) with `InterpFactor = 0`, and retail's
  own reconstruction keeps a zero-interp route at its seed matrix (CameraTask.cpp lines 39-41, 171-181): the camera does
  *not* follow. A route lasts its authored 60 frames; the leg it films takes ~3 s at the authored speeds (e.g. tag 23's
  first leg: Δ(4.71, −0.53) yalms at speed ref → 27 = 2.7 yps ⇒ 1.8 s). After the route expires kuluu holds its last frame,
  so the remainder of a beat films ground — `run5_rental_cs_dense/007_screenshot-7.png` (:45.6, route c01i ended :45.34)
  and `008_screenshot-8.png` (:47.2) are pixel-identical, empty corridor shots.
- From the mid-event mount edge (`:43.640`) onward neither player nor mount appears in any frame at all (run5 frames
  6/7/8; shot 6 is the authored fade-to-black), alongside WARN `load_mount_race race=32: body files resolved but
  unrendered [55325]`. That invisible-subject suspicion is now the top open item for these two shots and for the
  post-dismount complaints about the rented chocobo; it has not been diagnosed.

## Not established

1. **Why the two rental scenes look different.** The wire layer and cue choice are excluded (the
   streams are identical), and as of the second pass so is staging fidelity: every authored position,
   heading and walk goal lands in kuluu's stream (section above). One applied-side gap was found and
   fixed there (self-directed look-at); whether that alone closes the visual complaint needs one
   replay of both zones. Remaining candidates: attached-route camera behaviour — implemented, and its
   starts now log at INFO, but never frame-compared; note dat 30906 ships c00..c0b as a 12-way
   radius-4 orbit around body centre (attach=337 = mode 1 + locator 21 BODY_CENTER, interp=0 frozen
   single-point) while San d'Oria's conversation shots are world-space — that asymmetry is exactly why
   only Bastok exposes attach-path issues; and an unexecuted stage inside a routine the scene actually
   dispatches (every gap measured above lives in shared carriers, which is why none explains Bastok
   alone). One screenshot at the wrong moment, or one headless capture with per-actor transforms and
   camera state stamped at the recorded cue times, settles it. Narrowed further since: Bastok's own
   three 0x6E/0x99 beats are refusal-gated in retail (argument byte ≥ 0x80 — section above), so
   they cannot even be the difference; remaining candidates stay staging-visual ones.
2. **How retail turns an event animation id into motion content.** Superseded as a question, left
   open as a gap: the request slot takes a numeric id and the hand-off is class-gated; finding 2
   above names the three DLL-baked per-class id→index tables that sit behind it (addresses in
   Provenance), and nothing in any DAT or readable source names that index space. So kuluu's
   staging/pacing is complete and its content path for those ids stays closed until someone
   connects the indexes to a resource.
3. **Retail's composite order for its three screen-colour registers** (which register covers which,
   in which pass). Reading starting points named in Provenance; not read this pass.
4. **The tail of the event-animation chain.** Closed as to the store: the idle arm of the same
   action-state machine starts `res2` on the visual half and zeroes the action mark there, fed by
   the release-prep arm's `wait`/`res0` sequence (third-pass section above; addresses in
   Provenance). What remains: which states hand into the
   release-prep arm for each action value, and whether the run length is purely the handed-over routine's
   authored duration or also involves the per-frame counter. Moot for kuluu now (`OP_ANIMWAIT` is
   implemented on top of hold reports/deadlines), relevant again only if retail's exact release
   cadence ever needs matching.
5. **The colour-and-count stage tag** (the eleven `*dkn`/attach-staging sites): no source available
   to me states what it does — its payload is measured, its semantics are not.
6. **Which states hand into the release-prep arm for each action value** (item 4's residue): the
   entry edges of the machine's first dispatch are read; the per-value feed into release-prep is not.
7. **Mid-scene pose ordering at cue ingest (Bastok rental framing).** Superseded by the beat-graph pass above: kuluu's
   execution order is faithful to DAT byte order, so this is no longer the question; what remains of it is duration/framing
   (delta rows "shot length vs. walking-leg length" and the mount-visibility row). As of run4 the mechanism was
   narrowed to: kuluu consumes wire/snapshot actor positions as they land, so by `c00i` the player
   stands ~9 yalms into the exit path while retail's script keeps the teleport/legs behind different
   cuts; the attached seeds therefore compose shots of empty ground (run3 and run4 identical), plus an
   undiagnosed mid-event mount dispatch with `load_mount_race race=32 body files resolved but
   unrendered [55325]`. The unresolved site is where kuluu applies scene-driven player motion vs.
   where retail's script issues it relative to the camera calls — candidates: session snapshot POS
   application during locked-camera (`kuluu-session/src/session.rs` flood handling +
   `SceneAction::PlayerPosition`) and the mount-dispatch race against the scene. Named but not read;
   settle needs the transform-stamped capture described in the 2026-10-29 section.
8. **What file id 55325 is, and which of mount race 32's body slots resolves to it** (`MOUNT_BODY_SLOTS` /
   `load_mount_race` in `kuluu-render/src/ffxi_actor_render.rs`). The WARN means one resolved id yielded neither a read
   nor meshes while the load still returned Ok. If that file carries the chocobo's actual body, the rented mount renders
   invisible — the strongest remaining candidate for run5's actorless beats 2/3 and for the post-dismount complaints.
   Start by resolving the id to an install path (same walk `read_dat` uses) and dumping race 32's slot table.
9. **Whether retail stretches a camera route against the walking beat it films** (route `dur=60` frames vs ~1.8-3 s legs,
   measured above). The scheduler's speed-ratio scaling is on record for colour-drive stages only; nothing here says what
   it does to camera routes.

## Provenance

Build: generated C at `research/xi-on-anything/generated/all`, `build.json` build `2026-10-05`, game
path `C:\PhoenixXI\SquareEnix\FINAL FANTASY XI`. VA = RVA + `0x10000000`; all addresses below are VAs in that build.

Event VM handlers, re-read on this build (same addresses the retail-2026-09 record cites):
opcode 0x63 → `f_100b7550`, opcode 0x6E → `f_100b7c40`, opcode 0x99 → `f_100b7ce0`. Common shapes:
the event's exec pointer is the word at `[event+0x256]` (advanced by 3, 7 and 5 respectively), the
park/return flag is `[event+0x25A]`; actor table global VA `0x10480AF0`, indexed by a word index × 4;
word operands read via `f_100af4b0(event, n)` (index 1 for 0x63, index 5 for 0x6E); actor lookups via
`f_100afac0(event, 1, …)` with validity predicate `f_100afb00`.

- `f_100b7550`: owner from the word at `[event+2]`; null → exec += 3; busy on `[actor+0x1BC]` →
  `wr8([event+0x25A], 1)` with no advance; else `wr16([actor+0x1C0], operand)` and
  `wr8([actor+0x1BC], 9)`.
- `f_100b7c40`: gate on `[actor+0x120]` bit 9 (`shr ecx,9; test cl,1`) and `[actor+0x1BC]`; payload
  dword split by low byte (emote id) and arithmetic-shifted high byte (argument), passed with the
  lookup result and trailing `0, 1` to `f_100a0b60`, which tail-calls `f_100a0b90`.
- `f_100b7ce0`: same gate; parks on `[actor+0x1BC] != 0`, else exec += 5.
- The byte value 9 written into `[actor+0x1BC]` occurs at exactly one instruction image-wide
  (`100b7598`, inside `f_100b7550`).
- Actor action-state machine holding `[actor+0x1BC]`: `f_1008b460`. It is two chained switch dispatches
  on that byte, both read today:
  - first switch: case-lookup bytes at `0x1008C3DC` (indices are the action value), branch table at
    `0x1008C3B0`; state values it owns map to arms `1008b65d`(state 0) `1008b738` `1008b789` `1008b7be`
    `1008b8ee` `1008b937` `1008b9ce` `1008baef`, state 16 → `1008bbd0` (queries vtable slot `+0x2a0`
    with the fourcc `'coff'`, whose fall-through clears the mark at instr `1008bc2a` after
    `f_100d44b0(sub, 7)` and `f_10095680(0xff)`), state 17 → `1008b853`; states including **9** fall to
    its shared arm `1008bc40`, which is the second switch.
  - second switch (also at `L_1008bc40`): bounds check against 0x1a, case bytes at `0x1008C430`
    mapping state→case as 7→1, 8→2, **9→3**, 10→4, 11→5, 12→6, 13→7, and 20..26 → 8..0xe, everything
    else default; branch table at `0x1008C3F0`. The **state-9 arm begins at VA `1008BDDD`**:
    readiness predicate `f_100d12d0` on the visual half `[actor+0xa0]` (that predicate returns false
    when `[visual+0x8A4] != 0`), then on ready: `or [actor+0x128], 4`, push word `[actor+0x1C0]` and
    `call f_100d1c30(visual, id)`, then store state `0xa`.
  - state-10 arm at `1008be26`: clears bit 2 of `[actor+0x128]`; when byte `[visual+0xf8]` is zero it
    writes `'emot'`(0x746f6d65) into `[actor+0x1C4]`, the visual pointer into `[actor+0x1C8]`, calls
    vtable slot `+0x298` with that fourcc, and stores state `0xb`.
  - state-11 arm at `1008be89`: increments dword `[actor+0x1CC]`; while the named routine is running
    (vtable slot `+0x2a0` queried with the fourcc/argument pair from +0x1c4/+0x1c8, compared to 1) it
    keeps counting; on completion it calls `f_100d5f10(sub, 0, {0,1,4})` three times and zeroes a byte
    at `[actor+0x1bd]`; the counter is also compared against 4 in the not-running path.
- `f_100d1c30(visual, id)`: for `id < 0x40` and actor class (from `f_10084400`, values 0..7 through
  case bytes at `0x100D1C70` / branch table `0x100D1C68`) it stores `[visual+0x8A0] = id` and
  `[visual+0x8A4] = 3`; otherwise it returns without touching anything.
- The census of `[X+0x1bc]` accesses touches 130 functions; several are unrelated objects that happen
  to share the offset.
- Screen-colour register candidates found by constant search: neutral/identity words written as
  `wr32([X+0x338], 0x80808080)` at instr `1013d7f7` in `f_10013d790`, `0x80407090` at instr
  `1013b7bb`, and a sibling write near instr `1013bd54`; the neighbouring double-buffer copies in the
  XIClient reconstruction are `+0x31C ← +0x334`, `+0x320 ← +0x338`, `+0x324 ← +0x328`. Object identity
  of that triple is NOT established — located by constant search only, not traced to a singleton.
- DATs read directly (payload measurements): `ROM/62/110.DAT` (file_id 30904), `ROM/62/111.DAT`
  (30905), `ROM/62/112.DAT` (30906). Stage payload dwords are the stage's tag+8 dword, which
  `ffxi-dat/src/scheduler.rs` exposes as `SchedulerStage::id`; word counts are its `stage_words`.
- research/XIClient (`src/XIClient/source/Game/Scheduler/Tags/{0x0F,0x10,0x51,0x72}.cpp` and the
  matching headers under `include/Game/Scheduler/Tags/`, plus `Game/Screen/ScreenColorDriveTask.h`) —
  a community reconstruction, ranked below binary evidence; `HandleTag0x0E` there is an explicit
  unimplemented stub. Its colour-drive task fields are life/org-life floats, four source deltas, four
  destination bytes and a mode tag.
- Live cue-stream measurements: `artifacts/verify/bastok_chocobo_rental_summary.txt`,
  `artifacts/verify/chocobo_rental_summary.txt` (2026-10-28 runs of the two rental live tests).
- Emote-entry refusal (third-pass finding 1): every eligible 0x6E funnels through `f_100a0b60`,
  which tail-calls `f_100a0b90`; the refusal is that function's first instruction — instr
  `100a0b94`, `cmp dx, 0x61; jb` past the refusal body — returning `-1` when the
  (arithmetic-shifted, i.e. sign-extended) argument byte is ≥ 0x80.
- Numeric-id request tables (third-pass finding 2), as extracted: `f_100d1c30(visual, id)` stores
  `[visual+0x8A0] = id`, `[visual+0x8A4] = 3` for ids < 0x40, class-gated by `f_10084400`. The
  per-class emote-family tables are read with special-value tests against
  `0x7f/0x7e/0x7d/0x7c/0x7b/0x7a`; `0xfffe` = no entry:
  - class table A at VA `0x1035B6F0`: ids 4..9 → indexes 232..237, id 13 → 238, id 0x25 → 239.
  - class table B at `0x1035B7B8`: the same ids → 64..71 (a second, denser index space).
  - class table C at `0x1035B880`: dense — id→{0,1,5..24,26..40,48,56,80..88,125..127,256..278};
    read with an `[eax + ecx*8 + 0x88]`-style stride at instr `100a0e0a`, i.e. a per-entry record,
    not a fourcc.
- Chain-tail dispatch (third-pass finding 3), inside the actor action-state machine `f_1008b460`:
  first-switch lookup bytes at VA `0x1008C3DC`, branch table `[edx*4 + 0x1008C3B0]`; second-switch
  lookup at `0x1008C430`, branches `[+0x1008C3F0]` = {state7→`0x1008bd3b`, 8→`0x1008bd9a`,
  9→`0x1008bddd`, 10→`0x1008be26`, 11→`0x1008be89`, …}. The release-prep arm (state 7) gates on
  visual readiness `f_100d12d0`, checks the `'wait'` routine via vtable slot `+0x2a0`, starts
  `'res0'` via slot `+0x298` at instr `1008bd7e`, and stores state 8 at instr `1008bd84`. The idle
  arm (state 8) starts `'res2'` at instr `1008bdc1` and zeroes `[actor+0x1BC]` at instr `1008bdc7`.
  A third zero store sits at `1008c19a`, in a later second-switch arm.
- Bastok rental-thread emote/animwait beats in the zz-cs-threads dump (zone DAT example id 234,
  thread mask `7FFFFFF0`): opcode 0x6E near lines ~4894 / ~5086 / ~15644, each immediately followed
  by a 0x99; their ref-loaded operands resolve to 0x80F6 / 0x80FD / 0x8106 (refs-table words
  `+33014`).
- DLL image data (beyond generated C): the three numeric-id tables at VAs `0x1035B6F0`, `0x1035B7B8`,
  `0x1035B880` and the dispatch LUTs/branch tables of `f_1008b460` (`0x1008C3B0`/`0x1008C3DC`,
  `0x1008C3F0`/`0x1008C430`) were read as raw bytes out of
  `research/xi-on-anything/generated/FFXiMain.unpacked.dll` (pefile section mapping, VA→offset);
  that unpacked image is the one whose bytes match the generated C — the sibling `.retail.dll` does
  NOT match at those addresses.
- 2026-10-29 pass (dismount fix + run4): live hidden drives only (`KULUU_WINDOW_HIDDEN=1`, key-drive
  socket), never touching the user's session. Artifacts: `artifacts/verify/drive_1029/run4_client.log`
  (+ `run4_timeline.json`, frame dirs `run4_rental_cs` / `run4_npc_check` / `run4_dismount` /
  `run4_dismount_tail`), driver `C:/Users/Owner/.lmstudio/scratchpads/1v/drive_run4.py`. DAT stage
  dumps via `cargo run -q -p ffxi-dat --release --example dat-routine-stages -- <30906 c0 | 30904 fd
  | 30905 ch>`; EID height ground truth via `cargo run -p ffxi-dat --release --example zz-eid-refs`.
  Fixture state before the run: charid 5 reset to pos (43.882, 0.750, −108.629) zone 234, effects
  cleared (`docker exec cow-db mariadb -uxiadmin … xidb`).
- No process was executed, no DLL or generated file modified, nothing under `generated/` edited (the
  live drives above run the built client only, hidden-window, over the key-drive socket).
