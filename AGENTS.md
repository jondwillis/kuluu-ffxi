# AGENTS.md

Guidance for AI coding agents working in this repository. `CLAUDE.md` is a symlink to this file, so Claude Code and any AGENTS.md-aware tool read the same instructions.

## What this is

Kuluu is a faithful, open-source FINAL FANTASY XI **client** rebuilt in Rust + Bevy. It speaks the FFXI wire protocol to community-run private servers (LandSandBoat / Phoenix), **not** retail. It is **not a server**, and it **ships no game assets** — geometry/textures/audio/animation come from a user-provided retail install read at runtime from `FFXI_DAT_PATH` (default `vendor/game-files/SquareEnix/FINAL FANTASY XI`). Tables derived from LSB/POLUtils are baked in as compile-time constants, never as game content. The single source of truth for all durable work — including the grounded parity backlog — is beads (`bd ready`, label `roadmap`; see the Issue tracking section).

## Build, test, lint

`scripts/checks.sh` is the **single source of truth** for check commands — both the `pre-push` hook and CI call it, so they can't drift. Prefer it over spelling out cargo flags:

```bash
scripts/checks.sh fmt clippy            # what the pre-push hook runs
scripts/checks.sh fmt clippy test build # the full CI gate
cargo fmt --all                         # autofix formatting
```

`checks.sh` owns the full workspace invocations; don't restate them here. Everything compiles under one feature set, `--features native-window` (the default for `ffxi-client`/viewer) — match it for ad-hoc cargo runs so artifacts are reused across stages, e.g. a single test:

```bash
cargo test -p ffxi-proto framing::tests::roundtrip --features native-window
```

- **Nightly is required.** `rust-toolchain.toml` pins a dated nightly; the dev profile uses the Cranelift codegen backend (gated by `[unstable] codegen-backend` in `.cargo/config.toml`), which makes a *stable* cargo error out. Cranelift is dev-only — `--release` and the Steam Deck cross-build use LLVM.
- **Integration tests that need a live LSB server self-skip** when it's unreachable, so the test stage is safe on a network-isolated machine. Fixtures using `mysql_async` stamp out isolated accounts against a real MariaDB and only run when one is reachable.
- **Enable the hooks once per clone:** `cargo xtask install-hooks` (sets `core.hooksPath=.githooks`). Bypass a push with `git push --no-verify`; `PREPUSH_FAST=1 git push` runs fmt only.
- `xtask` is excluded from `default-members`, so plain `cargo build`/`test` skip it; run it via the `cargo xtask` alias.

## Running

Credentials and the DAT path come from env vars (never committed/logged). The launcher prompts for any unset credential and lists characters by name.

```bash
export FFXI_USER=... FFXI_PASS=... FFXI_CHAR="Exact Name" FFXI_SERVER=127.0.0.1
export FFXI_DAT_PATH="/path/to/SquareEnix/FINAL FANTASY XI"   # or: cargo xtask game

cargo run -p ffxi-client -- play                          # native window (default)
cargo run -p ffxi-client --no-default-features -- play --headless  # JSON event-stream agent session, no Bevy
```

`cargo xtask game [path|--copy|--download]` detects/validates/symlinks a retail install into `vendor/game-files/`. Client subcommands: `play`, `model-viewer`, `provision`, `create-char`.

## Issue tracking (beads)

Durable work items live in [beads](https://github.com/steveyegge/beads) (`bd`), a git-backed tracker checked into `.beads/`. Install with `brew install beads`. The dependency graph is a local Dolt db (gitignored); `.beads/issues.jsonl` is the diffable, PR-reviewable export — that's the file that crosses into git, so review it like code.

- `bd ready` — actionable work (open, unblocked). `bd list --status=in_progress` — active work.
- `bd create --title=… --description=… --type=feature --priority=2` — new issue. `bd update <id> --claim`, `bd close <id>`, `bd show <id>`.
- Priorities are `0`–`4` (0 = critical), **not** high/medium/low. Don't run `bd edit` (opens `$EDITOR`, blocks agents).

Beads is the **single source of truth for all durable work** (there is no `docs/ROADMAP.md` — that scoreboard was removed as a redundant hand-kept projection of beads). The grounded parity backlog is the `roadmap`-labelled beads, each citing `file:line` evidence and carrying `vanilla`/`enhanced` plus an area label (`hud`, `combat-action`, …). Pick work with `bd ready`. MEMORY.md auto-memory sits alongside beads and is **not** replaced by it — do **not** migrate it into `bd remember`.

GitHub Issues are a **generated projection of beads** for contributors, not a second source of truth. `scripts/beads-github-publish.py` is the outbound publisher: each in-scope bead maps to one issue keyed by a `<!-- beads-id: <id> -->` body marker, and it keeps the issue's title/body/managed-labels (`vanilla-parity`/`enhanced`/`area:*`/`status:*`) and open/closed state in sync — closing a bead closes its issue. `.github/workflows/beads-github-publish.yml` runs it automatically on every push to `main` that touches `.beads/issues.jsonl`, publishing **all** beads (not just `roadmap`); `workflow_dispatch` remains available for manual runs (`dry_run` defaults true there). Beads edits only reach GitHub once the auto-exported `.beads/issues.jsonl` is committed and pushed. The independent, opt-in inbound path is `scripts/beads-github-sync.sh` (imports GitHub issues *into* beads, keyed on `external_ref: gh-<number>`); the two directions are not a loop, so don't round-trip the same issues through both.

## Architecture

**The session runtime is async Tokio; the renderer is Bevy. They are decoupled by a wire-format snapshot.** This is the single most important thing to understand before touching cross-cutting code.

### Session pipeline (`ffxi-client`)

```
supervisor ──▶ reactor ──▶ session ──▶ {auth_client, lobby_client, map_client}
   reconnect/   200ms       protocol     LSB/Phoenix servers
   backoff      tick:       state +      (auth TLS, lobby, map UDP+Blowfish)
   + goal       keepalive,  SessionState
   persistence  follow,
                auto-attack
```

- `state.rs` — `SessionState` (the authoritative model: stage, entities, party, chat, inventory…) and the `AgentCommand`/`AgentEvent` channel vocabulary. Everything flows through tokio `mpsc`/`broadcast`/`watch`.
- `reactor.rs` — deterministic 200ms control loop (keepalive, follow-target, pathing, auto-attack, event auto-dismiss). High-level intent ("engage", "follow") comes from outside; per-tick movement does not.
- `supervisor.rs` — owns reconnect/backoff and goal persistence (`goal.json`).
- `map_client.rs` — the FFXI map-server transport: UDP, Blowfish, zlib packet (de)compression, the `0x` packet zoo decoded in `ffxi-proto`.

### Wire boundary (`ffxi-viewer-wire`)

`wire_translate.rs::state_to_snapshot` converts `SessionState` → `wire::SceneSnapshot`. **The same snapshot type feeds two consumers**: the in-process native viewer (`view_native/bridge.rs` polls a shared `Arc<Mutex<SessionState>>`) and the optional WebSocket `relay` (postcard frames consumed by `ffxi-viewer-wasm`, the browser build of the viewer). Keep `ffxi-viewer-wire` transport-agnostic; if you add a field to the scene, it crosses this boundary.

### Renderer (`ffxi-viewer-core` + `ffxi-client/src/view_native`)

Bevy systems: scene graph, chase camera + collision, HUD (`hud/`), minimap, picking, sky/weather, custom WGSL materials. Faithful rendering lives in dedicated materials — `FfxiZoneMaterial` (2× overbright vertex-lit zones), `skinned_ffxi` (PC/NPC skeletal meshes), point lights from Generator chunks. On macOS, Bevy's winit loop must own the OS main thread, so `main.rs` dispatches the GUI path specially under `native-window`.

### DAT + protocol parsing

- `ffxi-dat` — retail file parsers (VTABLE/FTABLE resolution, MZB/MMB zone+model geometry, ANI/skeleton animation, textures, weather, NPC names). Applies the **FFXI→Bevy coordinate transform**; get this wrong and geometry/actors render mirrored or sideways.
- `ffxi-proto` — the wire protocol (login, framing, blowfish, zlib, the `msg_*` packet families, autotranslate).
- `ffxi-actor` — skeleton + animation state (`actor_state`, `animation`, `skeleton_instance`) shared by the renderer for posing skinned meshes.
- `ffxi-audio` — BGW/SPW containers + ADPCM/PCM decode. `ffxi-nav` (in-house grid pathing) / `ffxi-nav-recast` (Recast/Detour navmesh from LSB xiNavmeshes).
- `ffxi-mcp` / `ffxi-agent` — MCP bridge + the LLM-agent harness. `ffxi-agent/CLAUDE.md` is a **runtime playbook for the agent**, not dev guidance for this repo.

### The LSB boundary is the critical correctness surface

Wire decoders/encoders, coord transforms, session-state transitions, shared numeric constants, and lifecycle assumptions are validated against an authoritative upstream (LandSandBoat). Source that crosses this boundary cites the upstream file in a comment (e.g. `vendor/server/...`, `vendor/Phoenix/...`). Two review agents exist specifically for it — `protocol-conformance-reviewer` (audit diffs against the authoritative source) and `lsb-invariant-prober` (propose unit tests pinning LSB invariants). Prefer them after non-trivial edits to `ffxi-proto/`, `session.rs`, `wire_translate.rs`, `map_client.rs`, `reactor.rs`, or `ffxi-nav-recast/`.

### Build-time vendor scrape (no hand-maintained tables)

`build.rs` in `ffxi-proto`/`ffxi-dat`/`ffxi-nav`/`ffxi-audio` reads LSB SQL/headers/lua and POLUtils XML out of `vendor/` and emits **compile-time Rust constants** (blowfish subkeys, zlib tables, msg/effect/job/spell/item names, zone-DAT id formulas, ROM file mappings). Never hand-copy these values — update the upstream pin and let the build regenerate them (see the `vendor-scrape` skill). The vendor submodules are **build-only**; nothing under `vendor/` (except a user's `game-files/`) is needed at runtime.

## Conventions

- **`vendor/` = build-time, used by the compiler. `research/` = read-only references** (XIM, AltanaViewer, Phoenix). Study upstream behavior and re-express it in our own code — do **not** copy source in. Both stay deinitialized unless you `git submodule update --init` them.
- **Vanilla parity is the default**; anything with no retail equivalent is Enhanced/addon (the `enhanced` label in beads), gated behind a feature flag.
- `ffxi-viewer-core` is `#![forbid(unsafe_code)]`. The workspace allows `clippy::type_complexity` and `clippy::too_many_arguments` (Bevy system signatures).
- Dev build-speed knobs live in `.cargo/config.toml` / `Cargo.toml` (Cranelift, `lld`, `dynamic_linking` feature). The macOS `CXXFLAGS` block works around a stale Command Line Tools layout for the Recast C++ bridge.
- **No magic numbers.** A literal that carries meaning — a threshold, scale, offset, frame rate — gets a named `const`, never an inline value. If it derives from upstream (LSB/POLUtils/XIM), scrape it at build time (the `vendor-scrape` skill); never hand-copy. If it's a deliberate tuning the data can't supply, name the `const` and let a one-line comment cite the WHY (e.g. `RETAIL_FPS` because retail runs at 30 fps; a `weather_opacity` table because the cloud generators ship no alpha keyframe to read). A literal that's a **contract between modules** — a wire tag, text marker, or format prefix one side *emits* and another *matches* — lives as an exported `const`/helper with the **emitter** and is imported by consumers; never re-type it (a locally-named copy in the consumer is still a second source), and pin the coupling with a guard test asserting the emitter still produces what the matcher expects.
- **No narrative code comments.** Names, types, and asserts carry WHAT/HOW; default to no comment. Keep one only for a WHY you can't encode, a citation to a vendor/protocol/spec source (the LSB-boundary convention), or a `// SAFETY:` justification. Doc comments (`///` `//!`) are *not* exempt — they rot and ramble like any prose, so keep them tight and accurate or prune them. The `comment-rot` hooks (`.claude/hooks/`) nudge on Edit and gate at Stop. The Stop nudge suggests a session-scoped bulk strip with `rmcm` (the `comment-remover` crate) — install it pinned via `scripts/install-tools.sh` (it's git-only at the version we use, so not on crates.io). `rmcm` strips *all* comments, including the doc/SAFETY/citation carve-outs, so use it only as a `--diff`-reviewed sweep, never wired to run automatically. (A more general, better-maintained alternative is `srgn` if the low-traffic crate becomes a concern.)
