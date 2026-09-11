# Applying this in Kuluu

## Contents
- Gates
- The LSB boundary is the thing to be careful about
- Repo rules that constrain a refactor
- Rust specifics that bite here
- Tracking
- Runtime verification

Repo-specific bindings for the portable method in `SKILL.md`. The rules below
come from `AGENTS.md`; where they conflict with anything here, `AGENTS.md` wins.

## Gates

`scripts/checks.sh` is the single source of truth — don't hand-spell cargo flags.

```bash
scripts/checks.sh harness fmt clippy            # pre-push
scripts/checks.sh harness fmt clippy test build # the CI gate
scripts/checks.sh doc                           # add when intra-doc links move
```

Capture the exit code explicitly; do not pipe the gate through `tail` and read
that status.

Everything compiles under one feature set, `--features native-window` — but
**only for crates that define it**. `ffxi-proto` has no `[features]` section at
all, so `cargo test -p ffxi-proto --features native-window` *errors*. That
mistake produced a silently-empty acceptance oracle that six commits reported
passing. For a featureless crate use `cargo test -p <crate> --locked`.

Nightly is pinned and the dev profile uses Cranelift; a stable cargo errors out.

## The LSB boundary is the thing to be careful about

`ffxi-proto/`, `session.rs`, `wire_translate.rs`, `map_client.rs`, `reactor.rs`
and `ffxi-nav-recast/` are validated against upstream LandSandBoat. Code
crossing this boundary cites its upstream file in a comment.

**Those citations are load-bearing.** They are frequently the only record of why
an offset or a mask is what it is. Moving code moves its citation verbatim;
deleting one to tidy up is a gate failure. A previously-planned refactor here was
correctly abandoned because generating the constants would have destroyed ~69
lines of citations and field-layout prose — a build-time guard was written
instead.

After non-trivial edits here, run the two dedicated reviewers:
`protocol-conformance-reviewer` (audit the diff against upstream) and
`lsb-invariant-prober` (propose tests pinning LSB invariants).

**The invariant most likely to be broken silently:** the c2s datagram header
must be the last subpacket's sync. Drift there means the server ignores every
client packet while the session still looks healthy.

## Repo rules that constrain a refactor

- **No narrative comments.** Names, types and asserts carry what/how. Keep a
  comment only for a why you cannot encode, a vendor/spec citation, or a
  `// SAFETY:`. Doc comments are held to the same bar. Never let a relocation
  introduce explanatory prose.
- **No magic numbers.** A literal carrying meaning gets a named const. A literal
  that is a *contract between modules* — a wire tag, a format prefix one side
  emits and another matches — lives as an exported const **with the emitter** and
  is imported by consumers. A locally-named copy in the consumer is still a
  second source. Pin the coupling with a guard test.
- **Never hand-copy upstream values.** If it derives from LSB or POLUtils, scrape
  it at build time (`vendor-scrape` skill).
- `kuluu-render` is `#![forbid(unsafe_code)]`.
- UI text under `launcher_ui/` must be printable ASCII.
- **Don't run `cargo fmt --all`** to autofix in a shared tree — it reformats the
  whole workspace including another session's in-progress files. Use
  `cargo fmt -p <crate>`.

## Rust specifics that bite here

- **`pub(super)` does not reach siblings.** In `text_input::menu::confirm` it
  means `pub(in text_input::menu)`, so a sibling `text_input::mouse_nav` calling
  it is E0603. This is why module directories here are **flat** — the in-repo
  precedent is `kuluu-render/src/hud/`, 40 flat files behind one `mod.rs`.
- **A parent's private items are visible to descendants** through `use super::*`,
  so moving a function into a child file often needs *no* visibility change at
  all. Compile before annotating.
- Nested glob re-exports resolve fine, so a moved `mod tests { use super::*; }`
  usually needs no import rewriting. Verify rather than assuming either way.
- Converting `op if op == CONST =>` guards to const patterns turns a duplicated
  arm into an `unreachable_patterns` error. Cheap, and one of the highest-value
  changes available in a big dispatch.

## Tracking

Beads is the source of truth; do not use markdown notes or a todo file.

Write results back to the bead — what landed, and **what the bead itself got
wrong**. Corrections re-derived during execution are the most valuable thing to
record, because otherwise the next reader repeats the derivation. Rejected
proposals belong in the bead too.

File follow-ups rather than folding them into a relocation commit. A behaviour
fix riding a relocation is exactly the failure the method exists to prevent.

Commit authority is granted — group finished work into coherent commits as you
go. **Confirm before `git push`.** In a tree that mixes another session's edits,
stage only your own hunks by explicit path; never `git add -A`.

## Runtime verification

Use the `verify` skill. Pick the surface by where the change is observable: wire
and session changes show up in the headless event stream and the live
integration tests; rendering, HUD and input changes only exist in pixels.

The live tests (`play_lifecycle`, `zone_change`, `agent_session`,
`delivery_box_live`) self-skip when no server is reachable — so confirm they
actually **ran** rather than skipped, or a green result means nothing.
