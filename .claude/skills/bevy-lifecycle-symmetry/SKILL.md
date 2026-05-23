---
name: bevy-lifecycle-symmetry
description: For any change that adds a new entity spawn site, Bevy Resource, or work that fires at a state-transition boundary (logout, zone change, disconnect, OnEnter/OnExit, AppPhase transition, session bridge, cleanup, despawn, drain), verify the symmetric cleanup is registered. Both layers — entity markers AND Resource drains — must be considered. Activates on lifecycle keywords: logout, disconnect, OnExit, OnEnter, AppPhase, spawn, despawn, drain, cleanup, lifecycle, session boundary.
---

# bevy-lifecycle-symmetry

This codebase has been bitten by lifecycle asymmetry: a clean operator
`/logout` left zone-decoration meshes rendering behind the launcher,
because `dat_mmb.rs`'s fresh-spawn branch forgot the `InGameEntity`
marker on the parent. Resources that cached per-zone state
(`MzbCollisionGeometry`, `LastAutoLoadedZone`, `TrackedEntities`,
`BgmSlots.active_entity`) survived too, breaking the next session.

The lesson: **every spawn site and every cache-holding Resource needs
a documented cleanup boundary, and both layers must be considered
together — the recursive despawn covers entity children, but never
reaches Resources.**

This skill walks the checklist when invoked on lifecycle-adjacent
work.

## Decision: does this change touch lifecycle?

Yes, run the checklist when the change:
- Adds a `commands.spawn(...)` (in `ffxi-viewer-core` or `ffxi-client`).
- Adds a `#[derive(Resource)]` or `init_resource::<T>()` for a type
  that holds cached / per-session / per-zone state.
- Adds a new system gated on `in_state(AppPhase::InGame)` (or any
  other state predicate) that *writes* shared state.
- Modifies code at `OnEnter(...)` / `OnExit(...)` schedules.
- Touches `bridge_connecting`, `despawn_ingame_entities`,
  `return_to_launcher_on_disconnect`, or the launcher state machine.
- Implements logout / disconnect / zone-change / kick paths.

Skip when the change is purely local logic (math, decoding, formatting)
that holds no Bevy resources or entities.

## Checklist: entity layer

For each new `commands.spawn(...)` introduced or modified:

1. **What's the entity's intended lifetime?**
   - Process lifetime → no marker.
   - Bounded by `AppPhase::InGame` → must attach `InGameEntity` to the
     *top-level* spawn. Children attached via `ChildOf(parent)`
     inherit recursive despawn — no need to tag each child.
   - Bounded by a narrower scope (zone-only, dialog-only,
     `AutoMzbOverlay`-style) → also tag with the broader
     `InGameEntity` AND the narrow sub-marker, so logout still
     reaches them even if the narrow despawn never fires.
2. **Is this the *parent* spawn?**
   - If yes: this is the only place the marker must go.
   - If no (it's a `ChildOf(parent)` spawn): the marker on the
     parent covers it. Verify the parent really has it.
3. **Are there multiple spawn branches in the same function?** Trap
   pattern: one branch handles "tracked entity" and another handles
   "fresh spawn", and only one branch was updated when `InGameEntity`
   was added later. Check every branch (`match` arms, `if/else`,
   `Option::map_or_else`).

Authoritative reference: `ffxi-viewer-core/src/dat_mmb.rs:677` (the
fresh-spawn parent that fixed the logout bug), and
`ffxi-viewer-core/src/dat_mzb.rs:1030` (the MZB parent that was always
correct).

## Checklist: resource layer

For each new `Resource` or modified Resource field:

1. **Does this hold per-session / per-zone / per-character state?**
   - `HashMap<u32, Entity>` (entity-id maps like `TrackedEntities`).
   - `Option<Entity>` for an active audio sink / active overlay.
   - `Option<u16>` last-zone trackers (`LastAutoLoadedZone`,
     `LastAtmosphereZone`).
   - `Vec<…>` baked from a specific zone's geometry
     (`MzbCollisionGeometry`).
   - `Option<Handle<_>>` cached asset handles keyed by zone or session.
   - `usize` cursors into shared event rings.
   - `VecDeque<ViewerEvent>` ring buffers (`EventLog.recent`).
   - **The snapshot itself** (`SceneState.snapshot`).

   If yes: this is a session-scoped Resource and needs a drain.

2. **Register the drain in
   `ffxi-client/src/view_native/mod.rs::despawn_ingame_entities`** —
   the canonical `OnExit(AppPhase::InGame)` handler. Add the resource
   to the system's parameters and clear/reset the relevant fields.
   `*r = T::default()` is fine for fully-stateless resets.

3. **Verify the drain doesn't break invariants the resource defines.**
   - `BgmSlots.install_root` is process-wide config, not session
     state — preserve it. Only `active_entity`, `active`, `tracks`,
     `event_cursor` need clearing.
   - Similarly, `Bindings`, `GraphicsSettings`, `DatRootRes`,
     `RuntimeHandle`, `LauncherClients` are process-lifetime.

4. **If the Resource holds an `Entity` handle and that entity is
   `InGameEntity`-tagged**, the despawn frees the entity but the
   handle in the Resource becomes a dangling reference. Always
   clear `Option<Entity>` fields explicitly.

## Checklist: state-boundary work

For systems registered at `OnEnter` / `OnExit` / state predicates:

1. **Is there a symmetric exit handler for everything OnEnter sets
   up?** Walk the file for the `OnEnter(...)` schedule and check
   each system in it has a matching `OnExit(...)` peer.
2. **Resources inserted by the *bridge* (`bridge_connecting`,
   `NativeSource`, `CommandTx`)**: these are owned by the App after
   first connect. Subsequent connects *replace* them via
   `insert_resource`, which is idempotent. No drain needed —
   they're not zone-keyed, they're session-keyed and the next
   session overwrites.
3. **Disconnect classification**: `view_native/mod.rs` distinguishes
   `DisconnectKind::Clean` (operator `/logout`,
   `reason = "server logout state=…"`) from `DisconnectKind::Forced`
   (kick, timeout, agent abort). Clean routes silently back to
   `Login` with creds intact; Forced populates `LoginErrorMsg`.
   When adding a new disconnect path, decide which kind it produces
   and verify the classifier in `classify_disconnect_reason`.

## Verification

After making a lifecycle-adjacent change, before declaring done:

1. **Build**: `cargo check -p ffxi-client -p ffxi-viewer-core`.
2. **Test classifier if you touched it**:
   `cargo test -p ffxi-client --bin ffxi-client --features native-window disconnect`.
3. **Manual smoke** (when an operator is available): launch, enter a
   zone with visible decorative geometry (Lower Jeuno, San d'Oria),
   `/logout`, confirm:
   - Zone props are gone (no buildings rendering behind launcher).
   - Returning to the same zone re-fires `LoadMzbRequest`
     (look for `vos2 bake` log lines on the new in-game).
   - BGM doesn't continue across the launcher screen.
4. **Concurrent-agent hazard**: if the work touched a load-bearing
   path (`InGameEntity` tagging, the drain system), commit
   immediately. Another Claude session in this repo may
   `git reset --hard HEAD` periodically.

## Anti-patterns to refuse

- "I'll just use `MmbOverlay` as the marker for both spawn-tagging
  and despawn-query." `MmbOverlay` describes *what* the entity is,
  not *when* it lives. The cleanup contract should be on `InGameEntity`
  (or a stricter sub-marker) regardless of type.
- "Bevy will GC the Resource when the entities it references are
  despawned." It will not. Resources persist for app lifetime
  unless explicitly cleared.
- "The next session will overwrite the cache on its first snapshot."
  Only true for `SceneState.snapshot` (fully replaced by the ingest
  system). Per-zone caches (`Last*Zone`) and entity-id maps
  (`TrackedEntities`) are *appended to / updated*, not replaced — a
  stale `Last*Zone` will prevent the auto-load from firing.
- "I'll add the drain in a follow-up commit." Drains are tightly
  coupled to the cache they drain — split commits lose the
  invariant. Same commit, every time.

## Counterexamples (process-wide, no drain needed)

| Resource | Why no drain |
|---|---|
| `Bindings`, `KeybindsStateRes` | User config, loaded from disk at startup; survives sessions intentionally. |
| `GraphicsSettings`, `GraphicsStateRes` | Same. |
| `DatRootRes`, `RuntimeHandle`, `LauncherClients`, `ServerInfo`, `SessionPorts` | Process-lifetime infrastructure; new session re-uses them. |
| `BgmSlots.install_root` | Resolved once from env; not session-keyed. |
| `LoginForm`, `Credentials` | Deliberately persisted across the launcher state machine so the operator doesn't re-type after disconnect; cleared only via `error_keyboard_system` on Forced disconnects. |
| `ZoneNameResolver`, `ZoneLineResolver`, `ZoneAtmosphereProvider` | Closures over compile-time tables; no per-session mutation. |

If a Resource matches one of these patterns, document *why* it's
process-wide in a comment near the `init_resource` call so future
sessions don't accidentally add a drain for it.
