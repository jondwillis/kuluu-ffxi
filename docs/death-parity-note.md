# Death / KO parity (observation note + fix log)

Source: voice/observation note 2026-06-12, watching self die and get returned to
the home point (Windurst Woods, zone 241) on a LandSandBoat server. Companion to
`render-feedback-gaps-note.md`. The unifying root cause for three of these is that
**a homepoint warp is a zone change, and in the renderer a zone change does not
cycle `AppPhase::InGame`** — so the clean-slate cleanup in `despawn_ingame_entities`
(`OnExit(InGame)`) never runs on a warp. Per-zone-change cleanup is scattered and
was incomplete.

## Observed gaps

1. **Death animation not performed.**
2. **Server homepoint timeout not surfaced** — the auto "return to home point" is
   server-mandated; we show no countdown for it.
3. **Pose not cleared after the warp** — the character keeps its pre-warp pose.
4. **Player spawns below the ground** on respawn.
5. **Music does not change** when the zone changes.

## Fixed (this pass)

- **#3 Pose not cleared** — `tick_live_ffxi_actors` reset its `ActorAnimInputs` on a
  zone change but never reset the `SkeletonAnimationCoordinator`, so the stale pose's
  animation slots survived. Now clears `actor.coordinator` + `actor.current_clip` so
  `advance_actor_pose` re-poses into idle from a clean slate.
  (`ffxi-viewer-core/src/ffxi_actor_render.rs`).
- **#4 Below-ground respawn** — `sync_entities_system` only updates X/Z for self and
  preserves Y (relying on the ground-snap raycast). The self entity is never
  despawned across a warp, so it kept the *previous* zone's Y; the down-only
  `ground_raycast` (ceiling = `current_y + 2.0`) can't recover when the new floor is
  above that stale height. Now reseeds self Y from the server position on the
  zone-change frame (`ffxi-viewer-core/src/scene.rs`).
- **#5 Music doesn't switch** — two bugs. (a) The warp keeps `AppPhase::InGame`, so
  `BgmSlots` was never drained; the death-music in the Dead slot (5) persisted and,
  while the KO icon lingered, `resolve_audible_slot` kept returning it so the new
  zone's slots 0/1 never played. (b) `drain_music_events_system` used a *positional*
  cursor into an `EventLog.recent` deque that front-pops at `EVENT_LOG_CAP = 64`, so
  once the zone-in flood saturated the buffer the cursor stuck and `MusicChanged`
  events were skipped. Now drains by the stable `pushed_total` global cursor and
  clears all slots on `ZoneChanged` (the new zone's `MusicNum`, which follows
  `ZoneChanged` in the queue, repopulates) (`ffxi-viewer-core/src/audio.rs`).
- **#1 (trigger only)** — the dead pose was gated on the self **entity** `hp_pct == 0`,
  but `npc_state` (which carries `animation`/`status`) is **only decoded for
  `CHAR_NPC`, never `CHAR_PC`** (`session.rs`), and the self entity's `hp_pct` only
  updates when CHAR_PC carries the UPDATE_HP flag. The death *prompt* already used the
  authoritative **party row** hp; the actor render now does too, so the player
  actually adopts the corpse pose on death.
- **#2 death countdown** — decode the `0x037` char_status packet (`GP_SERV_SERVERSTATUS`,
  new opcode const + `CharStatus` decoder in `ffxi-proto`). `dead_counter1` is at body
  offset **0x38** (u32 LE); `hpp` is bits 16..24 of `Flags0` (body 0x24).
  `seconds_until_homepoint = dead_counter1/60 - 360` (LSB pads `dead_counter1` with a
  fixed 6 min; the server's `CDeathState` force-warps at death + 60 min). Gated on the
  self packet **and `hpp == 0`** — `GetHPP()` clamps living HP to `max(1,…)`, so
  `hpp == 0` is a true KO sentinel (no alive false-positives; this matters because
  `dead_counter1` alone is identical for alive and fresh-dead). Plumbed
  `session.rs` → `AgentEvent::DeathTimerUpdated` → `SessionState.death_homepoint_secs`
  → wire snapshot → a `Home Point in M:SS` line in the death prompt
  (`hud/death_prompt.rs`), cleared on `ZoneChanged`. Offsets/formula confirmed by the
  `protocol-conformance-reviewer` against `vendor/server/.../char_status.cpp`,
  `charentity.cpp::GetTimeUntilDeathHomepoint`, `ai/states/death_state.cpp`.

## Open items

- **#1 fidelity** — the corpse pose resolves to `cor?` (`ffxi-actor` `idle_animation_id`,
  `dead && owner_is_none`) and is registered as a *looping idle*. Retail plays the
  death collapse **once** and holds the final corpse frame. Whether `cor?` is a
  collapse motion (then needs one-shot-hold) or a static pose needs a **live run** to
  see the clip behavior. The faithful server signal is `animation == ANIMATION_DEATH (3)`,
  but that field is never decoded for PCs today (see above).
- **#2 polish (deferred)** — the countdown now ticks down locally every second,
  anchored to the last `0x037` value and re-anchored when a fresh one arrives (the
  server only re-sends `0x037` on status changes). Still unverified on a **live run**:
  whether showing a numeric KO countdown is desirable as *vanilla* parity (retail shows
  the homepoint menu, not a visible clock) vs. flagging it Enhanced. The `0x00A LOGIN`
  `DeadCounter` (body offset **0xA0**, same encoding) is not yet decoded — only matters
  if you zone in while still KO'd.
